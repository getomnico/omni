#!/usr/bin/env python3
"""
Unit tests for the chunking functions.
"""
import re

import pytest
from transformers import AutoTokenizer
from processing import Chunker


@pytest.mark.unit
class TestChunkerSentenceMode:
    """Test cases for the Chunker class in sentence mode."""

    @pytest.fixture
    def tokenizer(self):
        """Load the tokenizer for testing."""
        return AutoTokenizer.from_pretrained(
            "jinaai/jina-embeddings-v3", trust_remote_code=True
        )

    @pytest.fixture
    def chunker(self):
        """Create a sentence chunker."""
        return Chunker()

    def test_single_sentence(self, tokenizer, chunker):
        """Test chunking a single sentence."""
        text = "This is a single sentence."
        token_spans, char_spans = chunker.chunk_by_sentences(text, 512, tokenizer)

        chunks = [text[start:end] for start, end in char_spans]

        assert len(chunks) == 1
        assert chunks[0].strip() == "This is a single sentence."

    def test_multiple_sentences_fit_in_chunk(self, tokenizer, chunker):
        """Test multiple sentences that fit in one chunk."""
        text = "First sentence. Second sentence. Third sentence."
        token_spans, char_spans = chunker.chunk_by_sentences(text, 512, tokenizer)

        chunks = [text[start:end] for start, end in char_spans]

        # All sentences should fit in one chunk with high token limit
        assert len(chunks) == 1
        assert "First sentence" in chunks[0]
        assert "Third sentence" in chunks[0]

    def test_small_chunk_size_creates_multiple_chunks(self, tokenizer, chunker):
        """Test that small chunk_size creates multiple chunks."""
        text = "First sentence. Second sentence. Third sentence. Fourth sentence."
        # Use small chunk size to force multiple chunks
        token_spans, char_spans = chunker.chunk_by_sentences(text, 10, tokenizer)

        chunks = [text[start:end] for start, end in char_spans]

        # Should create multiple chunks due to small token limit
        assert len(chunks) >= 2

    def test_empty_text(self, tokenizer, chunker):
        """Test chunking empty text."""
        text = ""
        token_spans, char_spans = chunker.chunk_by_sentences(text, 512, tokenizer)

        assert len(char_spans) == 0

    def test_text_without_periods(self, tokenizer, chunker):
        """Test text without sentence-ending periods."""
        text = "This text has no periods at all"
        token_spans, char_spans = chunker.chunk_by_sentences(text, 512, tokenizer)

        chunks = [text[start:end] for start, end in char_spans]

        # Should return the whole text as one chunk since no sentence boundaries
        assert len(chunks) == 1

    def test_span_annotations_validity(self, tokenizer, chunker):
        """Test that span annotations are valid token indices."""
        text = "First sentence. Second sentence. Third sentence."
        token_spans, char_spans = chunker.chunk_by_sentences(text, 512, tokenizer)

        # Tokenize the input to verify span validity
        inputs = tokenizer(text)
        token_count = len(inputs["input_ids"])

        for start_idx, end_idx in token_spans:
            assert 0 <= start_idx <= token_count
            assert 0 <= end_idx <= token_count
            assert start_idx < end_idx

    def test_chunk_respects_sentence_boundaries(self, tokenizer, chunker):
        """Test that chunks end at sentence boundaries."""
        text = "Short one. This is a medium length sentence with more words. This sentence is even longer and contains many more tokens. Tiny. Another medium sentence."

        # Use small token limit to force splitting
        token_spans, char_spans = chunker.chunk_by_sentences(text, 20, tokenizer)

        chunks = [text[start:end] for start, end in char_spans]

        # Each chunk should end with punctuation (sentences shouldn't be cut mid-way)
        for chunk in chunks:
            stripped = chunk.strip()
            assert (
                stripped.endswith(("."))
                or stripped.endswith(("!"))
                or stripped.endswith(("?"))
            )

    def test_long_single_sentence(self, tokenizer, chunker):
        """Test when a single sentence exceeds the chunk_size limit."""
        text = "This is an extremely long sentence that contains many words and will definitely exceed our token limit but since it is a single sentence it should still be kept together as one chunk despite being over the limit."

        # Test with token limit smaller than the sentence
        token_spans, char_spans = chunker.chunk_by_sentences(text, 20, tokenizer)

        chunks = [text[start:end] for start, end in char_spans]

        # Should create exactly one chunk (can't split a single sentence)
        assert len(chunks) == 1
        assert chunks[0].strip() == text.strip()

    def test_mixed_punctuation(self, tokenizer, chunker):
        """Test sentences with different punctuation marks."""
        text = "Is this working? Yes, it is! What about this. And this?"
        token_spans, char_spans = chunker.chunk_by_sentences(text, 512, tokenizer)

        chunks = [text[start:end] for start, end in char_spans]

        # All should fit in one chunk with high limit
        assert len(chunks) == 1
        assert "Is this working?" in chunks[0]
        assert "And this?" in chunks[0]


@pytest.mark.unit
class TestChunkerFixedMode:
    """Test cases for the Chunker class in fixed token mode."""

    @pytest.fixture
    def tokenizer(self):
        """Load the tokenizer for testing."""
        return AutoTokenizer.from_pretrained(
            "jinaai/jina-embeddings-v3", trust_remote_code=True
        )

    @pytest.fixture
    def chunker(self):
        """Create a fixed chunker."""
        return Chunker()

    def test_fixed_chunking_basic(self, tokenizer, chunker):
        """Test basic fixed token chunking."""
        text = "This is a test sentence that should be split into multiple chunks based on token count."
        token_spans, char_spans = chunker.chunk_by_tokens(text, 5, tokenizer)

        chunks = [text[start:end] for start, end in char_spans]

        # Should create multiple chunks with small token limit
        assert len(chunks) >= 2

    def test_fixed_chunking_covers_all_text(self, tokenizer, chunker):
        """Test that fixed chunking covers all text."""
        text = "The quick brown fox jumps over the lazy dog."
        token_spans, char_spans = chunker.chunk_by_tokens(text, 5, tokenizer)

        # Reconstruct text from chunks
        reconstructed = "".join([text[start:end] for start, end in char_spans])
        assert reconstructed == text


@pytest.mark.unit
class TestCharacterBasedChunking:
    """Test cases for character-based chunking functions."""

    def test_chunk_sentences_by_chars_basic(self):
        """Test basic character-based sentence chunking."""
        text = "First sentence. Second sentence. Third sentence."
        spans = Chunker.chunk_sentences_by_chars(text, 100)

        chunks = [text[start:end] for start, end in spans]

        # All should fit in one chunk with high limit
        assert len(chunks) == 1

    def test_chunk_sentences_by_chars_splits(self):
        """Test character-based sentence chunking with small limit."""
        text = "First sentence. Second sentence. Third sentence."
        spans = Chunker.chunk_sentences_by_chars(text, 20)

        chunks = [text[start:end] for start, end in spans]

        # Should create multiple chunks
        assert len(chunks) == 3

    def test_chunk_by_chars_basic(self):
        """Test basic character-based fixed chunking."""
        text = "This is a test string for chunking."
        spans = Chunker.chunk_by_chars(text, 10)

        chunks = [text[start:end] for start, end in spans]

        # Should create multiple chunks with small limit
        assert len(chunks) == 4

        # Reconstruct should match original
        reconstructed = "".join(chunks)
        assert reconstructed == text

    def test_chunk_by_chars_empty(self):
        """Test character-based chunking with empty text."""
        spans = Chunker.chunk_by_chars("", 10)
        assert len(spans) == 0

    def test_chunk_sentences_by_chars_oversized_no_boundaries(self):
        """Text without sentence boundaries exceeding max_chars must be split."""
        text = "word | " * 500  # ~3500 chars, no sentence boundaries
        max_chars = 1000
        spans = Chunker.chunk_sentences_by_chars(text, max_chars)

        chunks = [text[start:end] for start, end in spans]

        assert len(chunks) >= 2
        for chunk in chunks:
            assert len(chunk) <= max_chars

        reconstructed = "".join(chunks)
        assert reconstructed == text

    def test_chunk_sentences_by_chars_mixed_prose_and_table(self):
        """Sentences followed by a large block without boundaries."""
        prose = "First sentence. Second sentence. "
        table = "col1 | col2 | col3\n" * 200  # ~3800 chars, no sentence boundaries
        text = prose + table
        max_chars = 1000
        spans = Chunker.chunk_sentences_by_chars(text, max_chars)

        chunks = [text[start:end] for start, end in spans]

        for chunk in chunks:
            assert len(chunk) <= max_chars

        reconstructed = "".join(chunks)
        assert reconstructed == text

    def test_chunk_sentences_by_chars_never_strands_terminal_punctuation(self):
        """When an over-long unbroken run is split by the char fallback, its
        terminal punctuation must stay attached to the text instead of being
        emitted as a punctuation-only chunk."""
        text = "Lead sentence. " + ("x" * 200) + "."
        max_chars = 100
        spans = Chunker.chunk_sentences_by_chars(text, max_chars)

        chunks = [text[start:end] for start, end in spans]

        # Sanity: the over-long run had to be hard-split into several chunks.
        assert len(chunks) >= 3

        for chunk in chunks:
            stripped = chunk.strip()
            assert stripped, f"chunk is whitespace only: {chunk!r}"
            assert not re.fullmatch(r"[.!?]+", stripped), (
                f"terminal punctuation stranded in its own chunk: {chunk!r}"
            )

    def test_chunk_sentences_by_chars_boundaries_do_not_split_words(self):
        """Char fallback splits of an over-long run should fall between words
        when the text has word boundaries, and should not leave a chunk that
        starts with the separator whitespace."""
        run = "lorem ipsum dolor sit amet consectetur adipiscing elit " * 40
        text = "Short intro sentence. " + run  # over-long run with word boundaries
        max_chars = 500
        spans = Chunker.chunk_sentences_by_chars(text, max_chars)

        for start, end in spans:
            if start > 0:
                assert not text[start].isspace(), (
                    f"chunk begins with separator whitespace at {start}: "
                    f"{text[start:end][:40]!r}"
                )
                assert not (text[start - 1].isalnum() and text[start].isalnum()), (
                    f"chunk boundary splits a word at {start}: "
                    f"{text[start - 20 : start + 20]!r}"
                )
            if end < len(text):
                assert not (text[end - 1].isalnum() and text[end].isalnum()), (
                    f"chunk boundary splits a word at {end}: "
                    f"{text[end - 20 : end + 20]!r}"
                )


@pytest.mark.unit
class TestLongDocumentTwoLevelChunking:
    """Covers the two-level chunking used for long documents.

    Level 1 splits the document into sentence-aligned windows
    (Chunker.window_spans, as used by EmbeddingBatchProcessor). Level 2
    sentence-chunks each window via chunk_sentences_by_chars. The
    piece-relative chunk spans are then translated back into document offsets
    (offset + span) and stored; retrieval later slices the source document at
    those saved offsets. These tests check that the translated offsets point at
    clean boundaries in the source text.
    """

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

    # Mirrors the batch processor constants (window_size = EMBEDDING_MAX_MODEL_LEN
    # * 3), scaled down so the test stays fast.
    WINDOW_SIZE = 600
    MAX_CHARS = 250

    @staticmethod
    def _two_level_spans(text: str) -> list[tuple[int, int]]:
        """Translate window chunk spans back to document offsets, exactly like
        EmbeddingBatchProcessor does before storing chunk_start/end offsets."""
        max_chars = TestLongDocumentTwoLevelChunking.MAX_CHARS

        spans: list[tuple[int, int]] = []
        for offset, window_end in Chunker.window_spans(
            text, TestLongDocumentTwoLevelChunking.WINDOW_SIZE
        ):
            piece = text[offset:window_end]
            for start, end in Chunker.chunk_sentences_by_chars(piece, max_chars):
                spans.append((offset + start, offset + end))
        return spans

    def test_translated_spans_are_consistent_with_window_chunking(self):
        """Control: each translated span must slice the same text the window
        chunker embedded. The offset arithmetic itself is not the bug."""
        text = " ".join(s for _ in range(4) for s in self.SENTENCES)

        for offset, window_end in Chunker.window_spans(
            text, self.WINDOW_SIZE
        ):
            piece = text[offset:window_end]
            window_spans = Chunker.chunk_sentences_by_chars(piece, self.MAX_CHARS)
            for start, end in window_spans:
                assert text[offset + start : offset + end] == piece[start:end]

    def test_long_document_chunk_offsets_point_at_sentence_boundaries(self):
        """Chunk content is later recovered by slicing the source document at the
        saved start/end offsets, so each chunk must begin and end at a sentence
        boundary of the original text (not mid-word inside a sentence)."""
        text = " ".join(s for _ in range(4) for s in self.SENTENCES)
        spans = self._two_level_spans(text)

        assert len(spans) >= 6  # sanity: document spans multiple windows

        for start, end in spans:
            if start > 0:
                assert text[:start].rstrip().endswith((".", "!", "?")), (
                    f"chunk starts mid-sentence at {start}: {text[start:start + 40]!r}"
                )
            if end < len(text):
                assert text[:end].rstrip().endswith((".", "!", "?")), (
                    f"chunk ends mid-sentence at {end}: ...{text[end - 40 : end]!r}"
                )

    def test_long_document_chunk_offsets_do_not_split_words(self):
        """Window seams must not cut through a word in the source document, since
        the saved offsets are used verbatim to slice chunk content on retrieval."""
        text = " ".join(s for _ in range(4) for s in self.SENTENCES)
        spans = self._two_level_spans(text)

        for start, end in spans:
            if start > 0:
                assert not (text[start - 1].isalnum() and text[start].isalnum()), (
                    f"chunk starts mid-word at {start}: {text[start - 20 : start + 20]!r}"
                )
            if end < len(text):
                assert not (text[end - 1].isalnum() and text[end].isalnum()), (
                    f"chunk ends mid-word at {end}: {text[end - 20 : end + 20]!r}"
                )


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
