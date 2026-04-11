import logging
import time
import httpx
import asyncio

from . import EmbeddingProvider, Chunk
from processing import Chunker

logger = logging.getLogger(__name__)

OPENAI_MAX_BATCH_SIZE = 2048
OPENAI_MAX_RETRIES = 3
OPENAI_RETRY_DELAY = 1.0
CHARS_PER_TOKEN = 3


class OpenAIEmbeddingProvider(EmbeddingProvider):
    """
    Provider for OpenAI Embeddings API.

    Works with:
    - OpenAI's API (https://api.openai.com/v1)
    - Any OpenAI-compatible embeddings endpoint serving local models
    """

    def __init__(
        self,
        api_key: str,
        model: str,
        base_url: str,
        dimensions: int | None = None,
        max_model_len: int | None = None,
    ):
        self.api_key = api_key
        self.model = model
        self.base_url = base_url.rstrip("/")
        self.dimensions = dimensions
        self.max_model_len = max_model_len

        self.client = OpenAIEmbeddingClient(
            api_key=self.api_key,
            model=self.model,
            base_url=self.base_url,
            dimensions=self.dimensions,
        )

        logger.info(
            f"Initialized OpenAI embedding provider - model: {model}, base_url: {base_url}, max_model_len: {max_model_len}"
        )

    async def generate_embeddings(
        self,
        text: str,
        task: str,
        chunk_size: int | None,
        chunking_mode: str,
    ) -> list[Chunk]:
        """Generate embeddings using OpenAI-compatible API with chunking support."""
        return await self._generate_embeddings(text, task, chunk_size, chunking_mode)

    def get_model_name(self) -> str:
        """Get the name of the model being used."""
        return self.model

    async def _generate_embeddings(
        self,
        text: str,
        task: str,
        chunk_size: int | None,
        chunking_mode: str,
    ) -> list[Chunk]:
        """Generate embeddings with chunking support."""

        start_time = time.time()

        # Convert token-based sizes to chars for char-based chunking
        if chunk_size:
            max_chars = chunk_size * CHARS_PER_TOKEN
            if self.max_model_len:
                max_chars = min(max_chars, self.max_model_len * CHARS_PER_TOKEN)

        try:
            if chunking_mode == "none":
                t0 = time.monotonic()
                embeddings = await self.client.generate_embeddings([text])
                logger.debug(
                    f"Embedding API call: 1 text in {(time.monotonic() - t0) * 1000:.0f}ms"
                )
                chunks = [Chunk((0, len(text)), embeddings[0])]

            elif chunking_mode == "sentence":
                char_spans = Chunker.chunk_sentences_by_chars(text, max_chars)
                chunk_texts = [text[start:end] for start, end in char_spans]

                t0 = time.monotonic()
                embeddings = await self.client.generate_embeddings(chunk_texts)
                logger.debug(
                    f"Embedding API call: {len(chunk_texts)} texts in {(time.monotonic() - t0) * 1000:.0f}ms"
                )
                chunks = [
                    Chunk(span, embedding)
                    for span, embedding in zip(char_spans, embeddings)
                ]

            elif chunking_mode == "fixed":
                char_spans = Chunker.chunk_by_chars(text, max_chars)
                chunk_texts = [text[start:end] for start, end in char_spans]

                t0 = time.monotonic()
                embeddings = await self.client.generate_embeddings(chunk_texts)
                logger.debug(
                    f"Embedding API call: {len(chunk_texts)} texts in {(time.monotonic() - t0) * 1000:.0f}ms"
                )
                chunks = [
                    Chunk(span, embedding)
                    for span, embedding in zip(char_spans, embeddings)
                ]

            else:
                logger.warning(
                    f"Unsupported chunking mode: {chunking_mode}, using no chunking"
                )
                t0 = time.monotonic()
                embeddings = await self.client.generate_embeddings([text])
                logger.debug(
                    f"Embedding API call: 1 text in {(time.monotonic() - t0) * 1000:.0f}ms"
                )
                chunks = [Chunk((0, len(text)), embeddings[0])]

            total_time = time.time() - start_time
            logger.info(
                f"OpenAI embedding generation complete - total_time: {total_time:.2f}s, "
                f"total_chunks: {len(chunks)}"
            )

            return chunks

        except Exception as e:
            logger.error(f"Error generating embeddings with OpenAI: {str(e)}")
            raise Exception(f"OpenAI embedding generation failed: {str(e)}")


class OpenAIEmbeddingClient:
    """Client for OpenAI-compatible Embedding API."""

    def __init__(
        self,
        api_key: str,
        model: str,
        base_url: str,
        dimensions: int | None = None,
    ):
        self.api_key = api_key
        self.model = model
        self.base_url = base_url
        self.dimensions = dimensions
        self.embeddings_url = f"{base_url}/embeddings"

        self.client = httpx.AsyncClient(
            timeout=httpx.Timeout(60.0, connect=10.0),
            limits=httpx.Limits(max_keepalive_connections=5, max_connections=10),
        )

    async def close(self):
        """Close the HTTP client."""
        await self.client.aclose()

    async def _make_request(self, texts: list[str]) -> dict:
        """Make a request to the embeddings API with retry logic."""
        headers = {
            "Content-Type": "application/json",
            "Authorization": f"Bearer {self.api_key}",
        }

        payload = {
            "model": self.model,
            "input": texts,
        }

        if self.dimensions:
            payload["dimensions"] = self.dimensions

        for attempt in range(OPENAI_MAX_RETRIES):
            try:
                response = await self.client.post(
                    self.embeddings_url, headers=headers, json=payload
                )

                if response.status_code == 200:
                    return response.json()
                elif response.status_code == 429:
                    retry_after = float(
                        response.headers.get(
                            "Retry-After", OPENAI_RETRY_DELAY * (2**attempt)
                        )
                    )
                    logger.warning(
                        f"Rate limited, retrying after {retry_after} seconds. Response: {response.text}"
                    )
                    await asyncio.sleep(retry_after)
                else:
                    error_msg = (
                        f"OpenAI API error: {response.status_code} - {response.text}"
                    )
                    if attempt < OPENAI_MAX_RETRIES - 1:
                        logger.warning(f"{error_msg}, retrying...")
                        await asyncio.sleep(OPENAI_RETRY_DELAY * (2**attempt))
                    else:
                        raise Exception(error_msg)

            except httpx.RequestError as e:
                if attempt < OPENAI_MAX_RETRIES - 1:
                    logger.warning(f"Request error: {e}, retrying...")
                    await asyncio.sleep(OPENAI_RETRY_DELAY * (2**attempt))
                else:
                    raise Exception(
                        f"Failed to connect to OpenAI API {self.embeddings_url}: {e}"
                    )

        raise Exception(f"Failed after {OPENAI_MAX_RETRIES} retries")

    async def generate_embeddings(self, texts: list[str]) -> list[list[float]]:
        """Generate embeddings for a list of texts."""
        if not texts:
            return []

        all_embeddings = []

        for i in range(0, len(texts), OPENAI_MAX_BATCH_SIZE):
            batch = texts[i : i + OPENAI_MAX_BATCH_SIZE]

            logger.debug(
                f"Generating embeddings for batch {i // OPENAI_MAX_BATCH_SIZE + 1} ({len(batch)} texts)"
            )

            response = await self._make_request(batch)

            # Extract embeddings from response (sorted by index)
            sorted_data = sorted(response["data"], key=lambda x: x["index"])
            embeddings = [item["embedding"] for item in sorted_data]
            all_embeddings.extend(embeddings)

        return all_embeddings
