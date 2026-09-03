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
    def sentence_spans(text: str) -> list[tuple[int, int]]:
        """Split text into contiguous sentence spans.

        A sentence ends at terminal punctuation followed by whitespace, so each
        returned span starts where the previous one ended and covers all text.
        """
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

        return sentences

    @staticmethod
    def window_spans(text: str, window_size: int) -> list[tuple[int, int]]:
        """Split long text into sentence-aligned sub-chunk windows.

        Each window starts and ends on a sentence boundary of ``text``, so
        sentence-chunking a window never produces chunks that begin or end
        mid-sentence in the original text. A window may exceed ``window_size``
        only when it contains a single sentence longer than the limit.
        """
        if not text or window_size < 1:
            return []

        windows = []
        window_start = 0
        for sent_start, sent_end in Chunker.sentence_spans(text):
            if (
                sent_start > window_start
                and sent_end - window_start > window_size
            ):
                windows.append((window_start, sent_start))
                window_start = sent_start

        if window_start < len(text):
            windows.append((window_start, len(text)))

        return windows

    @staticmethod
    def _split_long_sentence(
        text: str, start: int, end: int, max_chars: int
    ) -> list[tuple[int, int]]:
        """Split a single sentence longer than max_chars into smaller pieces.

        Cuts prefer the last word start at or before the hard character limit so
        words are not split and no piece begins with the separating whitespace.
        Pieces that end up containing only punctuation or whitespace are folded
        into the previous piece so stray characters are not embedded on their
        own.
        """
        pieces = []
        pos = start
        while pos < end:
            limit = min(pos + max_chars, end)
            if limit >= end:
                pieces.append((pos, end))
                break

            # Prefer to cut at the last word start at or before the limit.
            cut = limit
            while cut > pos:
                if not text[cut].isspace() and text[cut - 1].isspace():
                    break
                cut -= 1
            if cut == pos:
                cut = limit  # no word boundary in range; hard cut required

            pieces.append((pos, cut))
            pos = cut

        cleaned = []
        for piece in pieces:
            if cleaned and not any(ch.isalnum() for ch in text[piece[0] : piece[1]]):
                cleaned[-1] = (cleaned[-1][0], piece[1])
            else:
                cleaned.append(piece)
        return cleaned

    @staticmethod
    def chunk_sentences_by_chars(text: str, max_chars: int) -> list[tuple[int, int]]:
        """Chunk text by sentences, keeping chunks under max_chars (character-based)."""
        if not text or max_chars < 1:
            return []

        sentences = Chunker.sentence_spans(text)

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

        final_chunks = []
        for start, end in chunks:
            if end - start > max_chars:
                final_chunks.extend(
                    Chunker._split_long_sentence(text, start, end, max_chars)
                )
            else:
                final_chunks.append((start, end))

        return final_chunks if final_chunks else [(0, len(text))]

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

    @staticmethod
    def _tokenize_with_offsets(text: str, tokenizer: AutoTokenizer):
        if hasattr(tokenizer, "encode_plus"):
            return tokenizer.encode_plus(
                text, return_offsets_mapping=True, add_special_tokens=False
            )
        return tokenizer(text, return_offsets_mapping=True, add_special_tokens=False)

    @staticmethod
    def _offset_mapping(tokens):
        mapping = getattr(tokens, "offset_mapping", None)
        if mapping is not None:
            return mapping
        return tokens["offset_mapping"]

    def chunk_by_tokens(
        self,
        text: str,
        chunk_size: int,
        tokenizer: AutoTokenizer,
    ) -> tuple[list[tuple[int, int]], list[tuple[int, int]]]:
        if not text or chunk_size < 1:
            return [], []

        self._check_text_length(text, tokenizer)

        tokens = self._tokenize_with_offsets(text, tokenizer)
        token_offsets = self._offset_mapping(tokens)

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

        tokens = self._tokenize_with_offsets(text, tokenizer)
        token_offsets = self._offset_mapping(tokens)

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
