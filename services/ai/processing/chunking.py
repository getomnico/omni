import asyncio
import multiprocessing
import re
from concurrent.futures import ThreadPoolExecutor
from typing import List, Tuple

from transformers import AutoTokenizer

# Shared executor for CPU-bound chunking operations
# HuggingFace tokenizers release the GIL during Rust tokenization,
# so ThreadPoolExecutor is more efficient than ProcessPoolExecutor
_chunking_max_workers = max(2, min(multiprocessing.cpu_count() - 1, 4))
_chunking_executor = ThreadPoolExecutor(
    max_workers=_chunking_max_workers, thread_name_prefix="chunker"
)


class Chunker:

    @staticmethod
    def chunk_sentences_by_chars(text: str, max_chars: int) -> list[tuple[int, int]]:
        """Chunk text by sentences, keeping chunks under max_chars (character-based)."""
        sentence_pattern = r"[.!?]+[\s]+"
        sentences = []
        last_end = 0

        for match in re.finditer(sentence_pattern, text):
            sentence_end = match.end()
            if last_end < sentence_end:
                sentences.append((last_end, sentence_end))
            last_end = sentence_end

        if last_end < len(text):
            sentences.append((last_end, len(text)))

        if not sentences:
            return [(0, len(text))]

        chunks = []
        chunk_start = 0
        last_sentence_end = 0

        for sent_start, sent_end in sentences:
            current_chunk_len = sent_end - chunk_start

            if current_chunk_len > max_chars and last_sentence_end > chunk_start:
                chunks.append((chunk_start, last_sentence_end))
                chunk_start = last_sentence_end

            last_sentence_end = sent_end

        if chunk_start < len(text):
            chunks.append((chunk_start, len(text)))

        return chunks if chunks else [(0, len(text))]

    @staticmethod
    def chunk_by_chars(text: str, max_chars: int) -> list[tuple[int, int]]:
        """Chunk text by fixed character count."""
        if not text or max_chars < 1:
            return []

        chunks = []
        for i in range(0, len(text), max_chars):
            chunk_end = min(i + max_chars, len(text))
            chunks.append((i, chunk_end))

        return chunks if chunks else [(0, len(text))]

    @staticmethod
    def _check_text_length(text: str, tokenizer: AutoTokenizer):
        max_len = getattr(tokenizer, "model_max_length", None)
        if max_len:
            # ~4 chars per token is a conservative estimate
            max_chars = max_len * 4
            if len(text) > max_chars:
                raise ValueError(
                    f"Text length ({len(text)} chars) exceeds estimated max for "
                    f"model sequence length of {max_len} tokens (~{max_chars} chars)"
                )

    def chunk_by_tokens(
        self,
        text: str,
        chunk_size: int,
        tokenizer: AutoTokenizer,
    ) -> tuple[list[tuple[int, int]], list[tuple[int, int]]]:
        if not text or chunk_size < 1:
            return [], []

        self._check_text_length(text, tokenizer)

        tokens = tokenizer.encode_plus(
            text, return_offsets_mapping=True, add_special_tokens=False
        )
        token_offsets = tokens.offset_mapping

        token_spans = []
        char_spans = []
        prev_char_end = 0  # Track end of previous chunk for contiguous spans

        for i in range(0, len(token_offsets), chunk_size):
            chunk_end = min(i + chunk_size, len(token_offsets))
            if chunk_end > i:  # Ensure valid span
                # Get character indices from token offsets
                # Use previous chunk end as start to include whitespace between tokens
                char_start = prev_char_end
                char_end = token_offsets[chunk_end - 1][1]

                # Validate character bounds
                if char_start < char_end and char_start >= 0 and char_end <= len(text):
                    token_spans.append((i, chunk_end))
                    char_spans.append((char_start, char_end))
                    prev_char_end = char_end

        return token_spans, char_spans

    def chunk_by_sentences(
        self,
        text: str,
        chunk_size: int,
        tokenizer: AutoTokenizer,
    ) -> Tuple[List[Tuple[int, int]], List[Tuple[int, int]]]:
        """Chunk text by sentences, keeping chunks under chunk_size tokens"""
        if not text or chunk_size < 1:
            return [], []

        self._check_text_length(text, tokenizer)

        tokens = tokenizer.encode_plus(
            text, return_offsets_mapping=True, add_special_tokens=False
        )
        token_offsets = tokens.offset_mapping

        if not token_offsets:
            return [], []

        token_spans = []
        char_spans = []
        chunk_start = 0
        last_sentence_end = 0

        for i in range(len(token_offsets)):
            # Check if this is a sentence boundary
            if (
                i < len(tokens.tokens(0))
                and tokens.tokens(0)[i] in (".", "!", "?")
                and (
                    (len(tokens.tokens(0)) == i + 1)
                    or (
                        i + 1 < len(token_offsets)
                        and tokens.token_to_chars(i).end
                        != tokens.token_to_chars(i + 1).start
                    )
                )
            ):
                # This is a sentence boundary
                sentence_end = i + 1
                current_chunk_tokens = sentence_end - chunk_start

                # Check if adding this sentence would exceed the limit
                if (
                    current_chunk_tokens > chunk_size
                    and last_sentence_end > chunk_start
                ):
                    # Create chunk up to the previous sentence
                    char_start = token_offsets[chunk_start][0]
                    char_end = token_offsets[last_sentence_end - 1][1]

                    if (
                        char_start < char_end
                        and char_start >= 0
                        and char_end <= len(text)
                    ):
                        token_spans.append((chunk_start, last_sentence_end))
                        char_spans.append((char_start, char_end))

                    # Start new chunk from the current sentence
                    chunk_start = last_sentence_end

                # Update last sentence end
                last_sentence_end = sentence_end

        # Handle the last chunk
        if chunk_start < len(token_offsets):
            char_start = token_offsets[chunk_start][0]
            char_end = token_offsets[-1][1]

            if char_start < char_end and char_start >= 0 and char_end <= len(text):
                token_spans.append((chunk_start, len(token_offsets)))
                char_spans.append((char_start, char_end))

        return token_spans, char_spans

    # -------------------------------------------------------------------------
    # Async wrappers for CPU-bound chunking operations
    # These offload tokenization to a thread pool to avoid blocking the event loop
    # -------------------------------------------------------------------------

    async def chunk_by_sentences_async(
        self,
        text: str,
        chunk_size: int,
        tokenizer: AutoTokenizer,
    ) -> Tuple[List[Tuple[int, int]], List[Tuple[int, int]]]:
        """Async version of chunk_by_sentences - runs in thread pool."""
        loop = asyncio.get_event_loop()
        return await loop.run_in_executor(
            _chunking_executor, self.chunk_by_sentences, text, chunk_size, tokenizer
        )

    async def chunk_by_tokens_async(
        self,
        text: str,
        chunk_size: int,
        tokenizer: AutoTokenizer,
    ) -> Tuple[List[Tuple[int, int]], List[Tuple[int, int]]]:
        """Async version of chunk_by_tokens - runs in thread pool."""
        loop = asyncio.get_event_loop()
        return await loop.run_in_executor(
            _chunking_executor, self.chunk_by_tokens, text, chunk_size, tokenizer
        )
