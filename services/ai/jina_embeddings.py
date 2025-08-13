import os
import logging
import time
import httpx
import asyncio
from typing import List, Optional, Tuple
from dataclasses import dataclass
import json

logger = logging.getLogger(__name__)

# JINA API Configuration
JINA_API_URL = os.getenv("JINA_API_URL", "https://api.jina.ai/v1/embeddings")
JINA_API_KEY = os.getenv("JINA_API_KEY", "")
JINA_MODEL = os.getenv("JINA_MODEL", "jina-embeddings-v3")
JINA_MAX_BATCH_SIZE = 2048  # JINA API supports up to 2048 texts per request
JINA_MAX_RETRIES = 3
JINA_RETRY_DELAY = 1.0  # Initial retry delay in seconds

# Task mappings for JINA API
QUERY_TASK = "retrieval.query"
PASSAGE_TASK = "retrieval.passage"
DEFAULT_TASK = PASSAGE_TASK


@dataclass
class Chunk:
    """Represents a text chunk with its character span and embedding"""
    span: Tuple[int, int]
    embedding: List[float]


class JINAEmbeddingClient:
    """Client for JINA AI Embedding API"""
    
    def __init__(self):
        self.api_url = JINA_API_URL
        self.api_key = JINA_API_KEY
        self.model = JINA_MODEL
        
        if not self.api_key:
            raise ValueError("JINA_API_KEY environment variable is not set")
        
        # Create async HTTP client with timeout settings
        self.client = httpx.AsyncClient(
            timeout=httpx.Timeout(30.0, connect=5.0),
            limits=httpx.Limits(max_keepalive_connections=5, max_connections=10)
        )
        
    async def close(self):
        """Close the HTTP client"""
        await self.client.aclose()
        
    async def _make_request(self, texts: List[str], task: str, dimensions: Optional[int] = None) -> dict:
        """Make a request to JINA API with retry logic"""
        headers = {
            "Content-Type": "application/json",
            "Authorization": f"Bearer {self.api_key}"
        }
        
        # Prepare request payload
        payload = {
            "model": self.model,
            "task": task,
            "input": texts,
        }
        
        # Add dimensions if specified (for Matryoshka representation)
        if dimensions:
            payload["dimensions"] = dimensions
            
        # Retry logic with exponential backoff
        for attempt in range(JINA_MAX_RETRIES):
            try:
                response = await self.client.post(
                    self.api_url,
                    headers=headers,
                    json=payload
                )
                
                if response.status_code == 200:
                    return response.json()
                elif response.status_code == 429:  # Rate limit
                    retry_after = float(response.headers.get("Retry-After", JINA_RETRY_DELAY * (2 ** attempt)))
                    logger.warning(f"Rate limited, retrying after {retry_after} seconds")
                    await asyncio.sleep(retry_after)
                else:
                    error_msg = f"JINA API error: {response.status_code} - {response.text}"
                    if attempt < JINA_MAX_RETRIES - 1:
                        logger.warning(f"{error_msg}, retrying...")
                        await asyncio.sleep(JINA_RETRY_DELAY * (2 ** attempt))
                    else:
                        raise Exception(error_msg)
                        
            except httpx.RequestError as e:
                if attempt < JINA_MAX_RETRIES - 1:
                    logger.warning(f"Request error: {e}, retrying...")
                    await asyncio.sleep(JINA_RETRY_DELAY * (2 ** attempt))
                else:
                    raise Exception(f"Failed to connect to JINA API: {e}")
                    
        raise Exception(f"Failed after {JINA_MAX_RETRIES} retries")
        
    async def generate_embeddings(
        self,
        texts: List[str],
        task: str = DEFAULT_TASK,
        dimensions: Optional[int] = None
    ) -> List[List[float]]:
        """Generate embeddings for a list of texts"""
        
        # Handle empty input
        if not texts:
            return []
            
        # Process in batches if necessary
        all_embeddings = []
        
        for i in range(0, len(texts), JINA_MAX_BATCH_SIZE):
            batch = texts[i:i + JINA_MAX_BATCH_SIZE]
            
            logger.info(f"Generating embeddings for batch {i//JINA_MAX_BATCH_SIZE + 1} ({len(batch)} texts)")
            
            response = await self._make_request(batch, task, dimensions)
            
            # Extract embeddings from response
            embeddings = [item["embedding"] for item in response["data"]]
            all_embeddings.extend(embeddings)
            
        return all_embeddings


def chunk_by_sentences(
    text: str,
    chunk_size: int = 512,
    overlap: int = 50
) -> List[Tuple[int, int]]:
    """
    Simple sentence-based chunking for text.
    Returns list of character spans for each chunk.
    """
    import re
    
    # Split text into sentences
    sentence_pattern = r'[.!?]+[\s]+'
    sentences = re.split(sentence_pattern, text)
    
    chunks = []
    current_chunk_start = 0
    current_chunk_text = ""
    
    for i, sentence in enumerate(sentences):
        # Skip empty sentences
        if not sentence.strip():
            continue
            
        # Check if adding this sentence exceeds chunk size
        if len(current_chunk_text) + len(sentence) > chunk_size and current_chunk_text:
            # Save current chunk
            chunk_end = current_chunk_start + len(current_chunk_text)
            chunks.append((current_chunk_start, chunk_end))
            
            # Start new chunk with overlap
            overlap_start = max(0, chunk_end - overlap)
            current_chunk_start = overlap_start
            current_chunk_text = text[overlap_start:chunk_end][-overlap:] if overlap > 0 else ""
            
        current_chunk_text += sentence
        if i < len(sentences) - 1:
            current_chunk_text += ". "  # Add sentence separator
            
    # Add the last chunk
    if current_chunk_text.strip():
        chunk_end = min(current_chunk_start + len(current_chunk_text), len(text))
        chunks.append((current_chunk_start, chunk_end))
        
    # If no chunks were created, return the entire text as one chunk
    if not chunks:
        chunks = [(0, len(text))]
        
    return chunks


def generate_sentence_chunks(
    text: str,
    k_sentences: int = 5
) -> List[Tuple[int, int]]:
    """
    Generate overlapping chunks of K consecutive sentences.
    Returns list of character spans for each chunk.
    """
    import re
    
    # Split text into sentences with their positions
    sentence_pattern = r'[.!?]+[\s]+'
    sentences = []
    last_end = 0
    
    for match in re.finditer(sentence_pattern, text):
        sentence_end = match.end()
        sentence_text = text[last_end:sentence_end].strip()
        if sentence_text:
            sentences.append((last_end, sentence_end))
        last_end = sentence_end
        
    # Add the last sentence if there's remaining text
    if last_end < len(text):
        remaining = text[last_end:].strip()
        if remaining:
            sentences.append((last_end, len(text)))
            
    # Generate chunks of k consecutive sentences
    chunks = []
    for i in range(0, len(sentences), k_sentences):
        chunk_sentences = sentences[i:i + k_sentences]
        if chunk_sentences:
            chunk_start = chunk_sentences[0][0]
            chunk_end = chunk_sentences[-1][1]
            chunks.append((chunk_start, chunk_end))
            
    # If no chunks were created, return the entire text as one chunk
    if not chunks:
        chunks = [(0, len(text))]
        
    return chunks


async def generate_embeddings_with_jina(
    texts: List[str],
    task: str = DEFAULT_TASK,
    chunk_size: int = 512,
    chunking_mode: str = "sentence",
    n_sentences: Optional[int] = None,
) -> List[List[Chunk]]:
    """
    Generate embeddings using JINA API with chunking support.
    This function matches the interface of generate_embeddings_sync from embeddings_v2.py
    """
    
    start_time = time.time()
    
    # Initialize JINA client
    client = JINAEmbeddingClient()
    
    try:
        logger.info(f"Starting JINA embedding generation for {len(texts)} texts")
        
        all_chunks = []
        
        for text in texts:
            if chunking_mode == "none":
                # No chunking - embed entire text
                logger.info(f"Skipping chunking for text (mode: none)")
                embeddings = await client.generate_embeddings([text], task)
                chunks = [Chunk((0, len(text)), embeddings[0])]
                
            elif chunking_mode == "sentence":
                # Sentence-based chunking
                if n_sentences:
                    # Use k-sentence chunking
                    chunk_spans = generate_sentence_chunks(text, n_sentences)
                else:
                    # Use size-based sentence chunking
                    chunk_spans = chunk_by_sentences(text, chunk_size)
                    
                # Also generate small overlapping chunks (5 sentences)
                small_chunk_spans = generate_sentence_chunks(text, k_sentences=5)
                
                # Combine all chunk spans
                all_spans = chunk_spans + small_chunk_spans
                
                # Extract text for each chunk
                chunk_texts = [text[start:end] for start, end in all_spans]
                
                # Generate embeddings for all chunks
                if chunk_texts:
                    embeddings = await client.generate_embeddings(chunk_texts, task)
                    chunks = [Chunk(span, embedding) for span, embedding in zip(all_spans, embeddings)]
                else:
                    # Fallback to entire text
                    embeddings = await client.generate_embeddings([text], task)
                    chunks = [Chunk((0, len(text)), embeddings[0])]
                    
            elif chunking_mode == "fixed":
                # Fixed-size chunking
                chunks_list = []
                for i in range(0, len(text), chunk_size):
                    chunk_text = text[i:i + chunk_size]
                    chunks_list.append((i, min(i + len(chunk_text), len(text))))
                    
                # Extract text for each chunk
                chunk_texts = [text[start:end] for start, end in chunks_list]
                
                # Generate embeddings
                embeddings = await client.generate_embeddings(chunk_texts, task)
                chunks = [Chunk(span, embedding) for span, embedding in zip(chunks_list, embeddings)]
                
            else:
                # Default to no chunking for unsupported modes
                logger.warning(f"Unsupported chunking mode: {chunking_mode}, using no chunking")
                embeddings = await client.generate_embeddings([text], task)
                chunks = [Chunk((0, len(text)), embeddings[0])]
                
            all_chunks.append(chunks)
            
        # Log processing time
        end_time = time.time()
        total_time = end_time - start_time
        total_chunks = sum(len(chunks_list) for chunks_list in all_chunks)
        logger.info(
            f"JINA embedding generation complete - total_time: {total_time:.2f}s, "
            f"total_chunks: {total_chunks}, chunks_per_text: {[len(c) for c in all_chunks]}"
        )
        
        return all_chunks
        
    except Exception as e:
        logger.error(f"Error generating embeddings with JINA: {str(e)}")
        raise Exception(f"JINA embedding generation failed: {str(e)}")
        
    finally:
        await client.close()


# Synchronous wrapper for compatibility
def generate_embeddings_sync(
    texts: List[str],
    task: str,
    chunk_size: int,
    chunking_mode: str,
    n_sentences: Optional[int] = None,
) -> List[List[Chunk]]:
    """
    Synchronous wrapper for JINA embeddings generation.
    This matches the interface of the original generate_embeddings_sync function.
    """
    # Run the async function in a new event loop
    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)
    try:
        return loop.run_until_complete(
            generate_embeddings_with_jina(
                texts, task, chunk_size, chunking_mode, n_sentences
            )
        )
    finally:
        loop.close()