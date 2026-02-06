import logging
import time
import httpx
import asyncio

from transformers import AutoTokenizer

from . import EmbeddingProvider, Chunk
from processing import Chunker

logger = logging.getLogger(__name__)

COHERE_MAX_BATCH_SIZE = 96
COHERE_MAX_RETRIES = 3
COHERE_RETRY_DELAY = 1.0


class CohereEmbeddingProvider(EmbeddingProvider):
    """Provider for Cohere Embeddings API."""

    TASK_MAP = {"query": "search_query", "passage": "search_document"}
    DEFAULT_TASK = "search_document"

    def __init__(
        self,
        api_key: str,
        model: str,
        api_url: str,
        max_model_len: int,
        dimensions: int | None = None,
    ):
        self.api_key = api_key
        self.model = model
        self.api_url = api_url
        self.max_model_len = max_model_len
        self.dimensions = dimensions

        if not self.api_key:
            raise ValueError("api_key is required for Cohere provider")

        self.client = CohereEmbeddingClient(
            self.api_key, self.model, self.api_url, self.dimensions
        )

        self.tokenizer = AutoTokenizer.from_pretrained("gpt2")
        self.chunker = Chunker()

        logger.info(
            f"Initialized Cohere embedding provider - model: {model}, max_model_len: {max_model_len}"
        )

    def _map_task(self, task: str) -> str:
        return self.TASK_MAP.get(task, task)

    def get_model_name(self) -> str:
        return self.model

    async def generate_embeddings(
        self,
        text: str,
        task: str,
        chunk_size: int,
        chunking_mode: str,
    ) -> list[Chunk]:
        start_time = time.time()

        effective_chunk_size = min(chunk_size, self.max_model_len)
        cohere_task = self._map_task(task)

        try:
            if chunking_mode == "none":
                embeddings = await self.client.generate_embeddings([text], cohere_task)
                chunks = [Chunk((0, len(text)), embeddings[0])]

            elif chunking_mode == "sentence":
                _, char_spans = await self.chunker.chunk_by_sentences_async(
                    text, effective_chunk_size, self.tokenizer
                )

                chunk_texts = [text[start:end] for start, end in char_spans]

                if chunk_texts:
                    embeddings = await self.client.generate_embeddings(
                        chunk_texts, cohere_task
                    )
                    chunks = [
                        Chunk(span, embedding)
                        for span, embedding in zip(char_spans, embeddings)
                    ]
                else:
                    embeddings = await self.client.generate_embeddings(
                        [text], cohere_task
                    )
                    chunks = [Chunk((0, len(text)), embeddings[0])]

            elif chunking_mode == "fixed":
                _, char_spans = await self.chunker.chunk_by_tokens_async(
                    text, effective_chunk_size, self.tokenizer
                )

                chunk_texts = [text[start:end] for start, end in char_spans]

                embeddings = await self.client.generate_embeddings(
                    chunk_texts, cohere_task
                )
                chunks = [
                    Chunk(span, embedding)
                    for span, embedding in zip(char_spans, embeddings)
                ]

            else:
                logger.warning(
                    f"Unsupported chunking mode: {chunking_mode}, using no chunking"
                )
                embeddings = await self.client.generate_embeddings([text], cohere_task)
                chunks = [Chunk((0, len(text)), embeddings[0])]

            end_time = time.time()
            total_time = end_time - start_time
            logger.info(
                f"Cohere embedding generation complete - total_time: {total_time:.2f}s, "
                f"total_chunks: {len(chunks)}"
            )

            return chunks

        except Exception as e:
            logger.error(f"Error generating embeddings with Cohere: {str(e)}")
            raise Exception(f"Cohere embedding generation failed: {str(e)}")


class CohereEmbeddingClient:
    """Client for Cohere Embedding API"""

    def __init__(
        self,
        api_key: str,
        model: str,
        api_url: str,
        dimensions: int | None = None,
    ):
        self.api_url = api_url
        self.api_key = api_key
        self.model = model
        self.dimensions = dimensions

        if not self.api_key:
            raise ValueError("Cohere API key is required")

        self.client = httpx.AsyncClient(
            timeout=httpx.Timeout(30.0, connect=5.0),
            limits=httpx.Limits(max_keepalive_connections=5, max_connections=10),
        )

    async def close(self):
        await self.client.aclose()

    async def _make_request(self, texts: list[str], input_type: str) -> dict:
        headers = {
            "Content-Type": "application/json",
            "Authorization": f"Bearer {self.api_key}",
        }

        payload = {
            "model": self.model,
            "texts": texts,
            "input_type": input_type,
            "embedding_types": ["float"],
            "truncate": "NONE",
        }

        if self.dimensions:
            payload["output_dimension"] = self.dimensions

        for attempt in range(COHERE_MAX_RETRIES):
            try:
                response = await self.client.post(
                    self.api_url, headers=headers, json=payload
                )

                if response.status_code == 200:
                    return response.json()
                elif response.status_code == 429:
                    retry_after = float(
                        response.headers.get(
                            "Retry-After", COHERE_RETRY_DELAY * (2**attempt)
                        )
                    )
                    logger.warning(
                        f"Rate limited, retrying after {retry_after} seconds"
                    )
                    await asyncio.sleep(retry_after)
                else:
                    error_msg = (
                        f"Cohere API error: {response.status_code} - {response.text}"
                    )
                    if attempt < COHERE_MAX_RETRIES - 1:
                        logger.warning(f"{error_msg}, retrying...")
                        await asyncio.sleep(COHERE_RETRY_DELAY * (2**attempt))
                    else:
                        raise Exception(error_msg)

            except httpx.RequestError as e:
                if attempt < COHERE_MAX_RETRIES - 1:
                    logger.warning(f"Request error: {e}, retrying...")
                    await asyncio.sleep(COHERE_RETRY_DELAY * (2**attempt))
                else:
                    raise Exception(f"Failed to connect to Cohere API: {e}")

        raise Exception(f"Failed after {COHERE_MAX_RETRIES} retries")

    async def generate_embeddings(
        self,
        texts: list[str],
        input_type: str = CohereEmbeddingProvider.DEFAULT_TASK,
    ) -> list[list[float]]:
        if not texts:
            return []

        all_embeddings = []

        for i in range(0, len(texts), COHERE_MAX_BATCH_SIZE):
            batch = texts[i : i + COHERE_MAX_BATCH_SIZE]

            logger.info(
                f"Generating embeddings for batch {i // COHERE_MAX_BATCH_SIZE + 1} ({len(batch)} texts)"
            )

            response = await self._make_request(batch, input_type)

            embeddings = response["embeddings"]["float"]
            all_embeddings.extend(embeddings)

        return all_embeddings
