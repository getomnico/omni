import asyncio
import logging
import time
import boto3
import json

from . import EmbeddingProvider, Chunk
from processing import chunk_by_sentences_chars, chunk_by_chars

logger = logging.getLogger(__name__)


class BedrockEmbeddingProvider(EmbeddingProvider):
    """Provider for AWS Bedrock Embeddings API (Amazon Titan)."""

    CHARS_PER_TOKEN = 3

    def __init__(self, model_id: str, region_name: str, max_model_len: int):
        self.model_id = model_id
        self.region_name = region_name
        self.max_model_len = max_model_len

        if not model_id:
            raise ValueError("model_id is required when using Bedrock model provider.")

        self.client = BedrockEmbeddingClient(self.model_id, self.region_name)

        logger.info(
            f"Initialized Bedrock embedding provider - model: {model_id}, max_model_len: {max_model_len}"
        )

    async def generate_embeddings(
        self,
        texts: list[str],
        task: str,
        chunk_size: int,
        chunking_mode: str,
    ) -> list[list[Chunk]]:
        """
        Generate embeddings using AWS Bedrock with chunking support.
        Runs in executor to avoid blocking the event loop with synchronous boto3 calls.
        """
        loop = asyncio.get_event_loop()
        return await loop.run_in_executor(
            None,  # Use default executor
            self._generate_embeddings_with_bedrock,
            texts,
            task,
            chunk_size,
            chunking_mode,
        )

    def get_model_name(self) -> str:
        """Get the name of the Bedrock model being used."""
        return self.model_id

    def _generate_embeddings_with_bedrock(
        self,
        texts: list[str],
        task: str,
        chunk_size: int,
        chunking_mode: str,
    ) -> list[list[Chunk]]:
        """Generate embeddings using AWS Bedrock with chunking support."""

        start_time = time.time()

        # Cap chunk_size at max_model_len and convert to chars
        effective_chunk_size = min(chunk_size, self.max_model_len)
        max_chars = effective_chunk_size * self.CHARS_PER_TOKEN

        try:
            logger.info(f"Starting Bedrock embedding generation for {len(texts)} texts")

            all_chunks = []

            for text in texts:
                if chunking_mode == "none":
                    embeddings = self.client.generate_embeddings([text])
                    chunks = [Chunk((0, len(text)), embeddings[0])]

                elif chunking_mode == "sentence":
                    char_spans = chunk_by_sentences_chars(text, max_chars)

                    chunk_texts = [text[start:end] for start, end in char_spans]

                    if chunk_texts:
                        embeddings = self.client.generate_embeddings(chunk_texts)
                        chunks = [
                            Chunk(span, embedding)
                            for span, embedding in zip(char_spans, embeddings)
                        ]
                    else:
                        embeddings = self.client.generate_embeddings([text])
                        chunks = [Chunk((0, len(text)), embeddings[0])]

                elif chunking_mode == "fixed":
                    char_spans = chunk_by_chars(text, max_chars)

                    chunk_texts = [text[start:end] for start, end in char_spans]

                    embeddings = self.client.generate_embeddings(chunk_texts)
                    chunks = [
                        Chunk(span, embedding)
                        for span, embedding in zip(char_spans, embeddings)
                    ]

                else:
                    logger.warning(
                        f"Unsupported chunking mode: {chunking_mode}, using no chunking"
                    )
                    embeddings = self.client.generate_embeddings([text])
                    chunks = [Chunk((0, len(text)), embeddings[0])]

                all_chunks.append(chunks)

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
    """Client for AWS Bedrock Embedding API."""

    MAX_RETRIES = 3
    RETRY_DELAY = 1.0

    def __init__(self, model_id: str, region_name: str):
        self.model_id = model_id
        self.region_name = region_name

        if not self.model_id:
            raise ValueError("model_id is required for Bedrock embeddings")

        if region_name:
            self.client = boto3.client("bedrock-runtime", region_name=region_name)
            logger.info(f"Created Bedrock client for region: {region_name}")
        else:
            self.client = boto3.client("bedrock-runtime")
            logger.info("Created Bedrock client with auto-detected region")

    def generate_embeddings(self, texts: list[str]) -> list[list[float]]:
        """Generate embeddings for a list of texts."""
        if not texts:
            return []

        all_embeddings = []

        for text in texts:
            native_request = {"inputText": text}
            request_body = json.dumps(native_request)

            for attempt in range(self.MAX_RETRIES):
                try:
                    response = self.client.invoke_model(
                        modelId=self.model_id, body=request_body
                    )

                    model_response = json.loads(response["body"].read())
                    embedding = model_response["embedding"]
                    all_embeddings.append(embedding)

                    if "inputTextTokenCount" in model_response:
                        logger.debug(
                            f"Input tokens: {model_response['inputTextTokenCount']}, Embedding size: {len(embedding)}"
                        )

                    break

                except Exception as e:
                    if attempt < self.MAX_RETRIES - 1:
                        logger.warning(f"Bedrock API error: {e}, retrying...")
                        time.sleep(self.RETRY_DELAY * (2**attempt))
                    else:
                        logger.error(
                            f"Failed to get embeddings from Bedrock after {self.MAX_RETRIES} retries: {e}"
                        )
                        raise Exception(f"Bedrock embedding failed: {e}")

        return all_embeddings
