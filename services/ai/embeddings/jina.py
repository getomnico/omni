import logging
import time
import httpx
import asyncio

from . import EmbeddingProvider, Chunk, chunk_by_sentences, generate_sentence_chunks

logger = logging.getLogger(__name__)

# JINA API Configuration
JINA_MAX_BATCH_SIZE = 2048  # JINA API supports up to 2048 texts per request
JINA_MAX_RETRIES = 3
JINA_RETRY_DELAY = 1.0  # Initial retry delay in seconds

# Task mappings for JINA API
QUERY_TASK = "retrieval.query"
PASSAGE_TASK = "retrieval.passage"
DEFAULT_TASK = PASSAGE_TASK


class JinaEmbeddingProvider(EmbeddingProvider):
    """Provider for JINA AI Embeddings API."""

    def __init__(self, api_key: str, model: str, api_url: str):
        self.api_key = api_key
        self.model = model
        self.api_url = api_url

        if not self.api_key:
            raise ValueError("api_key is required for JINA provider")

        self.client = JINAEmbeddingClient(self.api_key, self.model, self.api_url)

    async def generate_embeddings(
        self,
        texts: list[str],
        task: str,
        chunk_size: int,
        chunking_mode: str,
        n_sentences: int | None = None,
    ) -> list[list[Chunk]]:
        """
        Generate embeddings using JINA API with chunking support.
        """
        return await self._generate_embeddings_with_jina(
            texts, task, chunk_size, chunking_mode, n_sentences
        )

    def get_model_name(self) -> str:
        """Get the name of the JINA model being used."""
        return self.model

    async def _generate_embeddings_with_jina(
        self,
        texts: list[str],
        task: str = DEFAULT_TASK,
        chunk_size: int = 512,
        chunking_mode: str = "sentence",
        n_sentences: int | None = None,
    ) -> list[list[Chunk]]:
        """
        Generate embeddings using JINA API with chunking support.
        This function matches the interface of generate_embeddings_sync from embeddings_v2.py
        """

        start_time = time.time()

        try:
            logger.info(f"Starting JINA embedding generation for {len(texts)} texts")

            all_chunks = []

            for text in texts:
                if chunking_mode == "none":
                    # No chunking - embed entire text
                    logger.info(f"Skipping chunking for text (mode: none)")
                    embeddings = await self.client.generate_embeddings([text], task)
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
                        embeddings = await self.client.generate_embeddings(
                            chunk_texts, task
                        )
                        chunks = [
                            Chunk(span, embedding)
                            for span, embedding in zip(all_spans, embeddings)
                        ]
                    else:
                        # Fallback to entire text
                        embeddings = await self.client.generate_embeddings([text], task)
                        chunks = [Chunk((0, len(text)), embeddings[0])]

                elif chunking_mode == "fixed":
                    # Fixed-size chunking
                    chunks_list = []
                    for i in range(0, len(text), chunk_size):
                        chunk_text = text[i : i + chunk_size]
                        chunks_list.append((i, min(i + len(chunk_text), len(text))))

                    # Extract text for each chunk
                    chunk_texts = [text[start:end] for start, end in chunks_list]

                    # Generate embeddings
                    embeddings = await self.client.generate_embeddings(
                        chunk_texts, task
                    )
                    chunks = [
                        Chunk(span, embedding)
                        for span, embedding in zip(chunks_list, embeddings)
                    ]

                else:
                    # Default to no chunking for unsupported modes
                    logger.warning(
                        f"Unsupported chunking mode: {chunking_mode}, using no chunking"
                    )
                    embeddings = await self.client.generate_embeddings([text], task)
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


class JINAEmbeddingClient:
    """Client for JINA AI Embedding API"""

    def __init__(self, api_key: str, model: str, api_url: str):
        self.api_url = api_url
        self.api_key = api_key
        self.model = model

        if not self.api_key:
            raise ValueError("JINA API key is required")

        # Create async HTTP client with timeout settings
        self.client = httpx.AsyncClient(
            timeout=httpx.Timeout(30.0, connect=5.0),
            limits=httpx.Limits(max_keepalive_connections=5, max_connections=10),
        )

    async def close(self):
        """Close the HTTP client"""
        await self.client.aclose()

    async def _make_request(
        self, texts: list[str], task: str, dimensions: int | None = None
    ) -> dict:
        """Make a request to JINA API with retry logic"""
        headers = {
            "Content-Type": "application/json",
            "Authorization": f"Bearer {self.api_key}",
        }

        # Prepare request payload
        payload = {
            "model": self.model,
            "task": task,
            "input": texts,
            "late_chunking": True,
        }

        # Add dimensions if specified (for Matryoshka representation)
        if dimensions:
            payload["dimensions"] = dimensions

        # Retry logic with exponential backoff
        for attempt in range(JINA_MAX_RETRIES):
            try:
                response = await self.client.post(
                    self.api_url, headers=headers, json=payload
                )

                if response.status_code == 200:
                    return response.json()
                elif response.status_code == 429:  # Rate limit
                    retry_after = float(
                        response.headers.get(
                            "Retry-After", JINA_RETRY_DELAY * (2**attempt)
                        )
                    )
                    logger.warning(
                        f"Rate limited, retrying after {retry_after} seconds"
                    )
                    await asyncio.sleep(retry_after)
                else:
                    error_msg = (
                        f"JINA API error: {response.status_code} - {response.text}"
                    )
                    if attempt < JINA_MAX_RETRIES - 1:
                        logger.warning(f"{error_msg}, retrying...")
                        await asyncio.sleep(JINA_RETRY_DELAY * (2**attempt))
                    else:
                        raise Exception(error_msg)

            except httpx.RequestError as e:
                if attempt < JINA_MAX_RETRIES - 1:
                    logger.warning(f"Request error: {e}, retrying...")
                    await asyncio.sleep(JINA_RETRY_DELAY * (2**attempt))
                else:
                    raise Exception(f"Failed to connect to JINA API: {e}")

        raise Exception(f"Failed after {JINA_MAX_RETRIES} retries")

    async def generate_embeddings(
        self,
        texts: list[str],
        task: str = DEFAULT_TASK,
        dimensions: int | None = None,
    ) -> list[list[float]]:
        """Generate embeddings for a list of texts"""

        # Handle empty input
        if not texts:
            return []

        # Process in batches if necessary
        all_embeddings = []

        for i in range(0, len(texts), JINA_MAX_BATCH_SIZE):
            batch = texts[i : i + JINA_MAX_BATCH_SIZE]

            logger.info(
                f"Generating embeddings for batch {i//JINA_MAX_BATCH_SIZE + 1} ({len(batch)} texts)"
            )

            response = await self._make_request(batch, task, dimensions)

            # Extract embeddings from response
            embeddings = [item["embedding"] for item in response["data"]]
            all_embeddings.extend(embeddings)

        return all_embeddings
