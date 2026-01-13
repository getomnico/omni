import asyncio
import bisect
import logging
import multiprocessing
import re
from concurrent.futures import ThreadPoolExecutor
from typing import Dict, List, Optional, Tuple, Union

from llama_index.core.node_parser import SemanticSplitterNodeParser
from llama_index.core.schema import Document
from llama_index.embeddings.huggingface import HuggingFaceEmbedding
from transformers import AutoTokenizer

# Shared executor for CPU-bound chunking operations
# HuggingFace tokenizers release the GIL during Rust tokenization,
# so ThreadPoolExecutor is more efficient than ProcessPoolExecutor
# (no serialization overhead, shared memory for tokenizer models)
_chunking_max_workers = max(2, min(multiprocessing.cpu_count() - 1, 4))
_chunking_executor = ThreadPoolExecutor(
    max_workers=_chunking_max_workers, thread_name_prefix="chunker"
)

# Set the logging level to WARNING to suppress INFO and DEBUG messages
logging.getLogger("sentence_transformers").setLevel(logging.WARNING)

CHUNKING_STRATEGIES = ["semantic", "fixed", "sentence"]


def chunk_by_sentences_chars(text: str, max_chars: int) -> list[tuple[int, int]]:
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


def chunk_by_chars(text: str, max_chars: int) -> list[tuple[int, int]]:
    """Chunk text by fixed character count."""
    if not text or max_chars < 1:
        return []

    chunks = []
    for i in range(0, len(text), max_chars):
        chunk_end = min(i + max_chars, len(text))
        chunks.append((i, chunk_end))

    return chunks if chunks else [(0, len(text))]


class Chunker:
    def __init__(
        self,
        chunking_strategy: str,
    ):
        if chunking_strategy not in CHUNKING_STRATEGIES:
            raise ValueError("Unsupported chunking strategy: ", chunking_strategy)
        self.chunking_strategy = chunking_strategy
        self.embed_model = None
        self.embedding_model_name = None

    def _setup_semantic_chunking(self, embedding_model_name):
        if embedding_model_name:
            self.embedding_model_name = embedding_model_name

        self.embed_model = HuggingFaceEmbedding(
            model_name=self.embedding_model_name,
            trust_remote_code=True,
            embed_batch_size=1,
        )
        self.splitter = SemanticSplitterNodeParser(
            embed_model=self.embed_model,
            show_progress=False,
        )

    def chunk_semantically(
        self,
        text: str,
        tokenizer: "AutoTokenizer",
        embedding_model_name: Optional[str] = None,
    ) -> Tuple[List[Tuple[int, int]], List[Tuple[int, int]]]:
        if self.embed_model is None:
            self._setup_semantic_chunking(embedding_model_name)

        # Get semantic nodes
        nodes = [
            (node.start_char_idx, node.end_char_idx)
            for node in self.splitter.get_nodes_from_documents(
                [Document(text=text)], show_progress=False
            )
        ]

        # Tokenize the entire text
        tokens = tokenizer.encode_plus(
            text,
            return_offsets_mapping=True,
            add_special_tokens=False,
            padding=True,
            truncation=True,
        )
        token_offsets = tokens.offset_mapping

        token_spans = []
        char_spans = []

        for char_start, char_end in nodes:
            # Validate character bounds
            if char_start >= char_end or char_start < 0 or char_end > len(text):
                continue

            # Convert char indices to token indices
            start_chunk_index = bisect.bisect_left(
                [offset[0] for offset in token_offsets], char_start
            )
            end_chunk_index = bisect.bisect_right(
                [offset[1] for offset in token_offsets], char_end
            )

            # Validate token bounds and ensure start < end
            if (
                start_chunk_index < len(token_offsets)
                and end_chunk_index <= len(token_offsets)
                and start_chunk_index < end_chunk_index
            ):
                token_spans.append((start_chunk_index, end_chunk_index))
                char_spans.append((char_start, char_end))

        return token_spans, char_spans

    def chunk_by_tokens(
        self,
        text: str,
        chunk_size: int,
        tokenizer: "AutoTokenizer",
    ) -> Tuple[List[Tuple[int, int]], List[Tuple[int, int]]]:
        if not text or chunk_size < 1:
            return [], []

        # Pre-split very large texts to avoid tokenizer warnings/errors
        # Most tokenizers have max sequence length ~8192 tokens
        # Estimate ~4 chars per token, use 32K chars as safe limit (~8K tokens)
        MAX_CHARS_PER_TOKENIZATION = 32000

        if len(text) > MAX_CHARS_PER_TOKENIZATION:
            return self._chunk_large_text_by_tokens(
                text, chunk_size, tokenizer, MAX_CHARS_PER_TOKENIZATION
            )

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

    def _chunk_large_text_by_tokens(
        self,
        text: str,
        chunk_size: int,
        tokenizer: "AutoTokenizer",
        max_chars: int,
    ) -> Tuple[List[Tuple[int, int]], List[Tuple[int, int]]]:
        """Handle large texts by pre-splitting before tokenization.

        This avoids tokenizer warnings/errors when processing texts with millions of tokens.
        """
        # Split text into manageable pieces by characters
        char_pieces = chunk_by_chars(text, max_chars)

        all_token_spans = []
        all_char_spans = []
        cumulative_tokens = 0
        prev_char_end = 0

        for piece_start, piece_end in char_pieces:
            piece_text = text[piece_start:piece_end]

            if not piece_text:
                continue

            # Tokenize this piece
            tokens = tokenizer.encode_plus(
                piece_text, return_offsets_mapping=True, add_special_tokens=False
            )
            token_offsets = tokens.offset_mapping

            if not token_offsets:
                continue

            # Process this piece
            for i in range(0, len(token_offsets), chunk_size):
                chunk_end = min(i + chunk_size, len(token_offsets))
                if chunk_end > i:
                    # Get character indices from token offsets
                    char_start = prev_char_end
                    char_end = token_offsets[chunk_end - 1][1] + piece_start

                    if (
                        char_start < char_end
                        and char_start >= 0
                        and char_end <= len(text)
                    ):
                        all_token_spans.append(
                            (i + cumulative_tokens, chunk_end + cumulative_tokens)
                        )
                        all_char_spans.append((char_start, char_end))
                        prev_char_end = char_end

            cumulative_tokens += len(token_offsets)

        return all_token_spans, all_char_spans

    def chunk_by_sentences(
        self,
        text: str,
        chunk_size: int,
        tokenizer: "AutoTokenizer",
    ) -> Tuple[List[Tuple[int, int]], List[Tuple[int, int]]]:
        """Chunk text by sentences, keeping chunks under chunk_size tokens"""
        if not text or chunk_size < 1:
            return [], []

        # Pre-split very large texts to avoid tokenizer warnings/errors
        # Most tokenizers have max sequence length ~8192 tokens
        # Estimate ~4 chars per token, use 32K chars as safe limit (~8K tokens)
        MAX_CHARS_PER_TOKENIZATION = 32000

        if len(text) > MAX_CHARS_PER_TOKENIZATION:
            return self._chunk_large_text_by_sentences(
                text, chunk_size, tokenizer, MAX_CHARS_PER_TOKENIZATION
            )

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

    def _chunk_large_text_by_sentences(
        self,
        text: str,
        chunk_size: int,
        tokenizer: "AutoTokenizer",
        max_chars: int,
    ) -> Tuple[List[Tuple[int, int]], List[Tuple[int, int]]]:
        """Handle large texts by pre-splitting on sentence boundaries before tokenization.

        This avoids tokenizer warnings/errors when processing texts with millions of tokens.
        """
        # Split text into manageable pieces on sentence boundaries
        char_pieces = chunk_by_sentences_chars(text, max_chars)

        all_token_spans = []
        all_char_spans = []
        cumulative_tokens = 0

        for piece_start, piece_end in char_pieces:
            piece_text = text[piece_start:piece_end]

            if not piece_text.strip():
                continue

            # Tokenize this piece
            tokens = tokenizer.encode_plus(
                piece_text, return_offsets_mapping=True, add_special_tokens=False
            )
            token_offsets = tokens.offset_mapping

            if not token_offsets:
                continue

            # Process this piece using the same sentence-chunking logic
            piece_token_spans = []
            piece_char_spans = []
            piece_chunk_start = 0
            last_sentence_end = 0

            for i in range(len(token_offsets)):
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
                    sentence_end = i + 1
                    current_chunk_tokens = sentence_end - piece_chunk_start

                    if (
                        current_chunk_tokens > chunk_size
                        and last_sentence_end > piece_chunk_start
                    ):
                        char_start = token_offsets[piece_chunk_start][0]
                        char_end = token_offsets[last_sentence_end - 1][1]

                        if (
                            char_start < char_end
                            and char_start >= 0
                            and char_end <= len(piece_text)
                        ):
                            piece_token_spans.append(
                                (piece_chunk_start, last_sentence_end)
                            )
                            piece_char_spans.append((char_start, char_end))

                        piece_chunk_start = last_sentence_end

                    last_sentence_end = sentence_end

            # Handle last chunk in piece
            if piece_chunk_start < len(token_offsets):
                char_start = token_offsets[piece_chunk_start][0]
                char_end = token_offsets[-1][1]

                if (
                    char_start < char_end
                    and char_start >= 0
                    and char_end <= len(piece_text)
                ):
                    piece_token_spans.append((piece_chunk_start, len(token_offsets)))
                    piece_char_spans.append((char_start, char_end))

            # Adjust spans to be relative to original text
            for token_span, char_span in zip(piece_token_spans, piece_char_spans):
                # Token spans need cumulative offset
                all_token_spans.append(
                    (
                        token_span[0] + cumulative_tokens,
                        token_span[1] + cumulative_tokens,
                    )
                )
                # Char spans need piece offset
                all_char_spans.append(
                    (char_span[0] + piece_start, char_span[1] + piece_start)
                )

            cumulative_tokens += len(token_offsets)

        return all_token_spans, all_char_spans

    def chunk(
        self,
        text: str,
        tokenizer: "AutoTokenizer",
        chunking_strategy: str = None,
        chunk_size: Optional[int] = None,
        embedding_model_name: Optional[str] = None,
    ):
        chunking_strategy = chunking_strategy or self.chunking_strategy
        if chunking_strategy == "semantic":
            return self.chunk_semantically(
                text,
                embedding_model_name=embedding_model_name,
                tokenizer=tokenizer,
            )
        elif chunking_strategy == "fixed":
            if chunk_size < 4:
                raise ValueError("Chunk size must be >= 4.")
            return self.chunk_by_tokens(text, chunk_size, tokenizer)
        elif chunking_strategy == "sentence":
            return self.chunk_by_sentences(text, chunk_size, tokenizer)
        else:
            raise ValueError("Unsupported chunking strategy")

    # -------------------------------------------------------------------------
    # Async wrappers for CPU-bound chunking operations
    # These offload tokenization to a thread pool to avoid blocking the event loop
    # -------------------------------------------------------------------------

    async def chunk_by_sentences_async(
        self,
        text: str,
        chunk_size: int,
        tokenizer: "AutoTokenizer",
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
        tokenizer: "AutoTokenizer",
    ) -> Tuple[List[Tuple[int, int]], List[Tuple[int, int]]]:
        """Async version of chunk_by_tokens - runs in thread pool."""
        loop = asyncio.get_event_loop()
        return await loop.run_in_executor(
            _chunking_executor, self.chunk_by_tokens, text, chunk_size, tokenizer
        )

    async def chunk_semantically_async(
        self,
        text: str,
        tokenizer: "AutoTokenizer",
        embedding_model_name: Optional[str] = None,
    ) -> Tuple[List[Tuple[int, int]], List[Tuple[int, int]]]:
        """Async version of chunk_semantically - runs in thread pool."""
        loop = asyncio.get_event_loop()
        return await loop.run_in_executor(
            _chunking_executor,
            self.chunk_semantically,
            text,
            tokenizer,
            embedding_model_name,
        )

    async def chunk_async(
        self,
        text: str,
        tokenizer: "AutoTokenizer",
        chunking_strategy: str = None,
        chunk_size: Optional[int] = None,
        embedding_model_name: Optional[str] = None,
    ) -> Tuple[List[Tuple[int, int]], List[Tuple[int, int]]]:
        """Async version of chunk - routes to appropriate async chunking method."""
        chunking_strategy = chunking_strategy or self.chunking_strategy
        if chunking_strategy == "semantic":
            return await self.chunk_semantically_async(
                text,
                tokenizer=tokenizer,
                embedding_model_name=embedding_model_name,
            )
        elif chunking_strategy == "fixed":
            if chunk_size < 4:
                raise ValueError("Chunk size must be >= 4.")
            return await self.chunk_by_tokens_async(text, chunk_size, tokenizer)
        elif chunking_strategy == "sentence":
            return await self.chunk_by_sentences_async(text, chunk_size, tokenizer)
        else:
            raise ValueError("Unsupported chunking strategy")
