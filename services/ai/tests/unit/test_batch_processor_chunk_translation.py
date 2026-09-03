"""
Exercises EmbeddingBatchProcessor's chunk-index translation on the real
char-based chunking path.

For long documents the batch processor splits the content into
sentence-aligned windows (level 1) via Chunker.window_spans, sentence-chunks
each window with Chunker.chunk_sentences_by_chars (level 2), and translates the
piece-relative chunk spans back onto document offsets via ``offset +
chunk.span``. Those translated offsets are persisted as chunk_start_offset /
chunk_end_offset and are later used verbatim to slice chunk content out of the
source document during retrieval, so they must point at clean boundaries in the
original text.
"""

from unittest.mock import AsyncMock, MagicMock

import pytest

from embeddings import Chunk
from embeddings import batch_processor as bp
from embeddings.batch_processor import EmbeddingBatchProcessor
from processing import Chunker
from state import AppState

SENTENCES = [
    "The quick brown fox jumps over the lazy dog near the riverbank.",
    "Pack my box with five dozen liquor jugs of various sizes.",
    "How vexingly quick daft zebras jump when provoked by loud noises!",
    "Sphinx of black quartz, judge my vow and then decide.",
    "The five boxing wizards jump quickly while eating pancakes.",
    "Jackdaws love my big sphinx of quartz, a truly odd sight.",
    "Grumpy wizards make toxic brew for the evil Queen and Jack.",
    "Amazingly few discotheques provide jukeboxes these days, sadly.",
]


class SentenceChunkingProvider:
    """OpenAI/Cohere-style sentence-mode provider: char chunking per window.

    Records each window piece plus the piece-relative spans the chunker
    returned, so tests can verify how those spans are translated to document
    offsets by the processor.
    """

    def __init__(self, chunk_max_chars: int):
        self.chunk_max_chars = chunk_max_chars
        self.calls: list[tuple[str, list[tuple[int, int]]]] = []

    def get_model_name(self) -> str:
        return "test-embedding-model"

    async def generate_embeddings(
        self, text: str, task: str, chunk_size: int | None, chunking_mode: str
    ) -> list[Chunk]:
        spans = Chunker.chunk_sentences_by_chars(text, self.chunk_max_chars)
        self.calls.append((text, spans))
        return [Chunk((start, end), [0.1] * 8) for start, end in spans]


@pytest.mark.unit
class TestBatchProcessorChunkIndexTranslation:
    """Tests that the sub-chunk -> document offset translation performed by
    EmbeddingBatchProcessor yields offsets that slice clean chunk content."""

    @staticmethod
    async def _process_content(content: str) -> list[dict]:
        # Callers patch bp.EMBEDDING_MAX_MODEL_LEN to 200 so that the
        # sentence-aligned windows are capped at window_size = 200 * 3 = 600.
        provider = SentenceChunkingProvider(chunk_max_chars=250)

        state = AppState()
        state.embedding_provider = provider
        state.embedding_provider_type = "openai"
        state.content_storage = AsyncMock()
        state.content_storage.get_text = AsyncMock(return_value=content)

        embeddings_repo = AsyncMock()
        processor = EmbeddingBatchProcessor(
            documents_repo=AsyncMock(),
            queue_repo=AsyncMock(),
            embeddings_repo=embeddings_repo,
            app_state=state,
        )

        doc = MagicMock()
        doc.id = "doc-1"
        doc.content_id = "content-1"
        doc.external_id = "ext-1"

        item = MagicMock()
        item.id = "item-1"
        item.document_id = doc.id
        item.retry_count = 0

        await processor._process_single_document(item, doc)

        return embeddings_repo.bulk_insert.await_args.args[0]

    async def test_translated_offsets_match_window_chunk_spans(self, monkeypatch):
        """Control: the persisted offsets must be exactly the piece-relative
        chunk spans shifted by their window offset (offset + span). This pins
        the translation arithmetic in the real processor code."""
        monkeypatch.setattr(bp, "EMBEDDING_MAX_MODEL_LEN", 200)
        content = " ".join(s for _ in range(4) for s in SENTENCES)

        window_size = bp.EMBEDDING_MAX_MODEL_LEN * 3

        expected: list[tuple[int, int]] = []
        for offset, window_end in Chunker.window_spans(content, window_size):
            piece = content[offset:window_end]
            for start, end in Chunker.chunk_sentences_by_chars(piece, 250):
                expected.append((offset + start, offset + end))

        rows = await TestBatchProcessorChunkIndexTranslation._process_content(content)
        actual = [(r["chunk_start_offset"], r["chunk_end_offset"]) for r in rows]

        assert sorted(actual) == sorted(expected)

    async def test_stored_offsets_slice_clean_sentence_spans(self, monkeypatch):
        """Chunk content is later recovered by slicing the source document at
        the saved start/end offsets, so each stored offset pair must begin and
        end at a sentence boundary of the original text."""
        monkeypatch.setattr(bp, "EMBEDDING_MAX_MODEL_LEN", 200)
        content = " ".join(s for _ in range(4) for s in SENTENCES)
        rows = await TestBatchProcessorChunkIndexTranslation._process_content(content)

        assert len(rows) >= 6  # sanity: document spans multiple windows

        for row in rows:
            start, end = row["chunk_start_offset"], row["chunk_end_offset"]
            if start > 0:
                assert content[:start].rstrip().endswith((".", "!", "?")), (
                    f"stored chunk starts mid-sentence at {start}: "
                    f"{content[start : start + 40]!r}"
                )
            if end < len(content):
                assert content[:end].rstrip().endswith((".", "!", "?")), (
                    f"stored chunk ends mid-sentence at {end}: "
                    f"...{content[end - 40 : end]!r}"
                )
