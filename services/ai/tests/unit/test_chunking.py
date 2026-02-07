#!/usr/bin/env python3
"""
Unit tests for the chunking functions.
"""
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
        inputs = tokenizer(text, return_tensors="pt")
        token_count = inputs["input_ids"].shape[1]

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


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
