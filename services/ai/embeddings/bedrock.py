import logging
import time
import boto3
import json
from typing import List, Optional, Tuple

# Import common chunking utilities
from . import EmbeddingProvider, Chunk, chunk_by_sentences, generate_sentence_chunks

logger = logging.getLogger(__name__)

# Task mappings for Bedrock API
QUERY_TASK = "retrieval.query"
PASSAGE_TASK = "retrieval.passage"
DEFAULT_TASK = PASSAGE_TASK

# Bedrock constants
BEDROCK_MAX_RETRIES = 3
BEDROCK_RETRY_DELAY = 1.0


class BedrockEmbeddingProvider(EmbeddingProvider):
    """Provider for AWS Bedrock Embeddings API (Amazon Titan)."""

    def __init__(self, model_id: str, region_name: str):
        self.model_id = model_id
        self.region_name = region_name

        if not model_id:
            raise ValueError("model_id is required when using Bedrock model provider.")

        self.client = BedrockEmbeddingClient(self.model_id, self.region_name)

    def generate_embeddings_sync(
        self,
        texts: List[str],
        task: str,
        chunk_size: int,
        chunking_mode: str,
        n_sentences: Optional[int] = None,
    ) -> List[List[Chunk]]:
        """
        Generate embeddings using AWS Bedrock with chunking support.
        """
        # Create client instance with our configuration
        result = self._generate_embeddings_with_bedrock(
            texts, task, chunk_size, chunking_mode, n_sentences
        )
        return result

    def get_model_name(self) -> str:
        """Get the name of the Bedrock model being used."""
        return self.model_id

    def _generate_embeddings_with_bedrock(
        self,
        texts: List[str],
        task: str = DEFAULT_TASK,
        chunk_size: int = 512,
        chunking_mode: str = "sentence",
        n_sentences: Optional[int] = None,
    ) -> List[List[Chunk]]:
        """
        Generate embeddings using AWS Bedrock with chunking support.
        This function matches the interface of generate_embeddings_sync from embeddings_v2.py
        """

        start_time = time.time()

        try:
            logger.info(f"Starting Bedrock embedding generation for {len(texts)} texts")

            all_chunks = []

            for text in texts:
                if chunking_mode == "none":
                    # No chunking - embed entire text
                    logger.info(f"Skipping chunking for text (mode: none)")
                    embeddings = self.client.generate_embeddings([text], task)
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
                        embeddings = self.client.generate_embeddings(chunk_texts, task)
                        chunks = [Chunk(span, embedding) for span, embedding in zip(all_spans, embeddings)]
                    else:
                        # Fallback to entire text
                        embeddings = self.client.generate_embeddings([text], task)
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
                    embeddings = self.client.generate_embeddings(chunk_texts, task)
                    chunks = [Chunk(span, embedding) for span, embedding in zip(chunks_list, embeddings)]

                else:
                    # Default to no chunking for unsupported modes
                    logger.warning(f"Unsupported chunking mode: {chunking_mode}, using no chunking")
                    embeddings = self.client.generate_embeddings([text], task)
                    chunks = [Chunk((0, len(text)), embeddings[0])]

                all_chunks.append(chunks)

            # Log processing time
            end_time = time.time()
            total_time = end_time - start_time
            total_chunks = sum(len(chunks_list) for chunks_list in all_chunks)
            logger.info(
                f"Bedrock embedding generation complete - total_time: {total_time:.2f}s, "
                f"total_chunks: {total_chunks}, chunks_per_text: {[len(c) for c in all_chunks]}"
            )

            return all_chunks

        except Exception as e:
            logger.error(f"Error generating embeddings with Bedrock: {str(e)}")
            raise Exception(f"Bedrock embedding generation failed: {str(e)}")


class BedrockEmbeddingClient:
    """Client for AWS Bedrock Embedding API"""

    def __init__(self, model_id: str, region_name: str):
        self.model_id = model_id
        self.region_name = region_name

        if not self.model_id:
            raise ValueError("model_id is required for Bedrock embeddings")

        # Create Bedrock Runtime client
        if region_name:
            self.client = boto3.client("bedrock-runtime", region_name=region_name)
            logger.info(f"Created Bedrock client for region: {region_name}")
        else:
            # Auto-detect region from ECS environment
            self.client = boto3.client("bedrock-runtime")
            logger.info("Created Bedrock client with auto-detected region")

    def generate_embeddings(self, texts: List[str], task: str = DEFAULT_TASK) -> List[List[float]]:
        """Generate embeddings for a list of texts"""

        # Handle empty input
        if not texts:
            return []

        all_embeddings = []

        for text in texts:
            # Create the request for the model
            native_request = {"inputText": text}

            # Convert the native request to JSON
            request_body = json.dumps(native_request)

            # Retry logic with exponential backoff
            for attempt in range(BEDROCK_MAX_RETRIES):
                try:
                    # Invoke the model with the request
                    response = self.client.invoke_model(
                        modelId=self.model_id,
                        body=request_body
                    )

                    # Decode the model's native response body
                    model_response = json.loads(response["body"].read())

                    # Extract the generated embedding
                    embedding = model_response["embedding"]
                    all_embeddings.append(embedding)

                    # Log token count if available
                    if "inputTextTokenCount" in model_response:
                        logger.debug(f"Input tokens: {model_response['inputTextTokenCount']}, Embedding size: {len(embedding)}")

                    break  # Success, exit retry loop

                except Exception as e:
                    if attempt < BEDROCK_MAX_RETRIES - 1:
                        logger.warning(f"Bedrock API error: {e}, retrying...")
                        time.sleep(BEDROCK_RETRY_DELAY * (2 ** attempt))
                    else:
                        logger.error(f"Failed to get embeddings from Bedrock after {BEDROCK_MAX_RETRIES} retries: {e}")
                        raise Exception(f"Bedrock embedding failed: {e}")

        return all_embeddings

