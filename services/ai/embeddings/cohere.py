import logging
import time

import cohere

from . import EmbeddingProvider, Chunk
from processing import Chunker

logger = logging.getLogger(__name__)


class CohereEmbeddingProvider(EmbeddingProvider):
    """Provider for Cohere Embeddings API."""

    MAX_BATCH_SIZE = 96
    CHARS_PER_TOKEN = 3
    TASK_MAP = {"query": "search_query", "passage": "search_document"}

    def __init__(
        self,
        api_key: str,
        model: str,
        api_url: str | None,
        max_model_len: int,
        dimensions: int | None = None,
    ):
        self.api_key = api_key
        self.model = model
        self.max_model_len = max_model_len
        self.dimensions = dimensions

        if not self.api_key:
            raise ValueError("api_key is required for Cohere provider")

        self.client = cohere.AsyncClientV2(
            api_key=self.api_key,
            base_url=api_url or None,
        )

        logger.info(
            f"Initialized Cohere embedding provider - model: {model}, max_model_len: {max_model_len}"
        )

    def _map_task(self, task: str) -> str:
        return self.TASK_MAP.get(task, task)

    def get_model_name(self) -> str:
        return self.model

    async def _embed_texts(
        self, texts: list[str], input_type: str
    ) -> list[list[float]]:
        if not texts:
            return []

        all_embeddings: list[list[float]] = []

        for i in range(0, len(texts), self.MAX_BATCH_SIZE):
            batch = texts[i : i + self.MAX_BATCH_SIZE]
            logger.info(
                f"Generating embeddings for batch {i // self.MAX_BATCH_SIZE + 1} ({len(batch)} texts)"
            )

            kwargs: dict = {
                "model": self.model,
                "input_type": input_type,
                "embedding_types": ["float"],
                "texts": batch,
                "truncate": "NONE",
            }
            if self.dimensions:
                kwargs["output_dimension"] = self.dimensions

            response = await self.client.embed(**kwargs)
            all_embeddings.extend(response.embeddings.float_)

        return all_embeddings

    async def generate_embeddings(
        self,
        text: str,
        task: str,
        chunk_size: int,
        chunking_mode: str,
    ) -> list[Chunk]:
        start_time = time.time()

        max_chars = min(chunk_size, self.max_model_len) * self.CHARS_PER_TOKEN
        cohere_task = self._map_task(task)

        try:
            if chunking_mode == "none":
                embeddings = await self._embed_texts([text], cohere_task)
                chunks = [Chunk((0, len(text)), embeddings[0])]

            elif chunking_mode == "sentence":
                char_spans = Chunker.chunk_sentences_by_chars(text, max_chars)
                chunk_texts = [text[start:end] for start, end in char_spans]

                embeddings = await self._embed_texts(chunk_texts, cohere_task)
                chunks = [
                    Chunk(span, embedding)
                    for span, embedding in zip(char_spans, embeddings)
                ]

            elif chunking_mode == "fixed":
                char_spans = Chunker.chunk_by_chars(text, max_chars)
                chunk_texts = [text[start:end] for start, end in char_spans]

                embeddings = await self._embed_texts(chunk_texts, cohere_task)
                chunks = [
                    Chunk(span, embedding)
                    for span, embedding in zip(char_spans, embeddings)
                ]

            else:
                logger.warning(
                    f"Unsupported chunking mode: {chunking_mode}, using no chunking"
                )
                embeddings = await self._embed_texts([text], cohere_task)
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
