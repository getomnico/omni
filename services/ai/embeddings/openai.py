import logging
import time
import httpx
import asyncio

from . import EmbeddingProvider, Chunk, chunk_by_sentences, generate_sentence_chunks

logger = logging.getLogger(__name__)

OPENAI_MAX_BATCH_SIZE = 2048
OPENAI_MAX_RETRIES = 3
OPENAI_RETRY_DELAY = 1.0


class OpenAIEmbeddingProvider(EmbeddingProvider):
    """
    Provider for OpenAI Embeddings API.

    Works with:
    - OpenAI's API (https://api.openai.com/v1)
    - vLLM server serving local models
    """

    def __init__(
        self,
        api_key: str,
        model: str,
        base_url: str = "https://api.openai.com/v1",
        dimensions: int | None = None,
    ):
        self.api_key = api_key
        self.model = model
        self.base_url = base_url.rstrip("/")
        self.dimensions = dimensions

        self.client = OpenAIEmbeddingClient(
            api_key=self.api_key,
            model=self.model,
            base_url=self.base_url,
            dimensions=self.dimensions,
        )

        logger.info(
            f"Initialized OpenAI embedding provider - model: {model}, base_url: {base_url}"
        )

    async def generate_embeddings(
        self,
        texts: list[str],
        task: str,
        chunk_size: int,
        chunking_mode: str,
        n_sentences: int | None = None,
    ) -> list[list[Chunk]]:
        """Generate embeddings using OpenAI-compatible API with chunking support."""
        return await self._generate_embeddings(
            texts, task, chunk_size, chunking_mode, n_sentences
        )

    def get_model_name(self) -> str:
        """Get the name of the model being used."""
        return self.model

    async def _generate_embeddings(
        self,
        texts: list[str],
        task: str,
        chunk_size: int = 512,
        chunking_mode: str = "sentence",
        n_sentences: int | None = None,
    ) -> list[list[Chunk]]:
        """Generate embeddings with chunking support."""

        start_time = time.time()

        try:
            logger.info(f"Starting OpenAI embedding generation for {len(texts)} texts")

            all_chunks = []

            for text in texts:
                if chunking_mode == "none":
                    embeddings = await self.client.generate_embeddings([text])
                    chunks = [Chunk((0, len(text)), embeddings[0])]

                elif chunking_mode == "sentence":
                    if n_sentences:
                        chunk_spans = generate_sentence_chunks(text, n_sentences)
                    else:
                        chunk_spans = chunk_by_sentences(text, chunk_size)

                    # Also generate small overlapping chunks (5 sentences)
                    small_chunk_spans = generate_sentence_chunks(text, k_sentences=5)

                    # Combine all chunk spans
                    all_spans = chunk_spans + small_chunk_spans

                    chunk_texts = [text[start:end] for start, end in all_spans]

                    if chunk_texts:
                        embeddings = await self.client.generate_embeddings(chunk_texts)
                        chunks = [
                            Chunk(span, embedding)
                            for span, embedding in zip(all_spans, embeddings)
                        ]
                    else:
                        embeddings = await self.client.generate_embeddings([text])
                        chunks = [Chunk((0, len(text)), embeddings[0])]

                elif chunking_mode == "fixed":
                    chunks_list = []
                    for i in range(0, len(text), chunk_size):
                        chunk_text = text[i : i + chunk_size]
                        chunks_list.append((i, min(i + len(chunk_text), len(text))))

                    chunk_texts = [text[start:end] for start, end in chunks_list]

                    embeddings = await self.client.generate_embeddings(chunk_texts)
                    chunks = [
                        Chunk(span, embedding)
                        for span, embedding in zip(chunks_list, embeddings)
                    ]

                else:
                    logger.warning(
                        f"Unsupported chunking mode: {chunking_mode}, using no chunking"
                    )
                    embeddings = await self.client.generate_embeddings([text])
                    chunks = [Chunk((0, len(text)), embeddings[0])]

                all_chunks.append(chunks)

            end_time = time.time()
            total_time = end_time - start_time
            total_chunks = sum(len(chunks_list) for chunks_list in all_chunks)
            logger.info(
                f"OpenAI embedding generation complete - total_time: {total_time:.2f}s, "
                f"total_chunks: {total_chunks}, chunks_per_text: {[len(c) for c in all_chunks]}"
            )

            return all_chunks

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
                        f"Rate limited, retrying after {retry_after} seconds"
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
