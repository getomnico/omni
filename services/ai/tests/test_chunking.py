#!/usr/bin/env python3
"""
Unit tests for the chunking functions in main.py
"""
import pytest
import torch
from transformers import AutoTokenizer
from chunking import Chunker


class TestChunkBySentences:
    """Test cases for the chunk_by_sentences function"""

    @pytest.fixture
    def tokenizer(self):
        """Load the tokenizer for testing"""
        return AutoTokenizer.from_pretrained(
            "jinaai/jina-embeddings-v3", trust_remote_code=True
        )

    @pytest.fixture
    def chunker(self):
        """Create a sentence chunker"""
        return Chunker("sentence")

    def test_single_sentence(self, tokenizer, chunker):
        """Test chunking a single sentence"""
        text = "This is a single sentence."
        token_spans, char_spans = chunker.chunk(text, tokenizer, n_sentences=1)

        # Extract actual text chunks using char spans
        chunks = [text[start:end] for start, end in char_spans]

        assert len(chunks) == 1
        assert len(char_spans) == 1
        assert chunks[0].strip() == "This is a single sentence."

    def test_multiple_sentences(self, tokenizer, chunker):
        """Test chunking multiple sentences"""
        text = "First sentence. Second sentence. Third sentence."
        token_spans, char_spans = chunker.chunk(text, tokenizer, n_sentences=1)

        # Extract actual text chunks using char spans
        chunks = [text[start:end] for start, end in char_spans]

        assert len(chunks) == 3
        assert len(char_spans) == 3
        assert "First sentence" in chunks[0]
        assert "Second sentence" in chunks[1]
        assert "Third sentence" in chunks[2]

    def test_sentences_with_spacing(self, tokenizer, chunker):
        """Test sentences with various spacing"""
        text = "First sentence.  Second sentence.   Third sentence."
        token_spans, char_spans = chunker.chunk(text, tokenizer, n_sentences=1)

        # Extract actual text chunks using char spans
        chunks = [text[start:end] for start, end in char_spans]

        assert len(chunks) == 3
        assert len(char_spans) == 3
        # Verify that spacing is preserved appropriately
        for chunk in chunks:
            assert len(chunk.strip()) > 0

    def test_empty_text(self, tokenizer, chunker):
        """Test chunking empty text"""
        text = ""
        token_spans, char_spans = chunker.chunk(text, tokenizer, n_sentences=1)

        # Extract actual text chunks using char spans
        chunks = [text[start:end] for start, end in char_spans]

        assert len(chunks) == 0
        assert len(char_spans) == 0

    def test_text_without_periods(self, tokenizer, chunker):
        """Test text without sentence-ending periods"""
        text = "This text has no periods at all"
        token_spans, char_spans = chunker.chunk(text, tokenizer, n_sentences=1)

        # Extract actual text chunks using char spans
        chunks = [text[start:end] for start, end in char_spans]

        # Should return the whole text as one chunk since no sentence boundaries are found
        assert len(chunks) == 1
        assert len(char_spans) == 1

    def test_complex_punctuation(self, tokenizer, chunker):
        """Test sentences with complex punctuation"""
        text = "Dr. Smith went to the U.S.A. He visited N.Y.C. Then he returned."
        token_spans, char_spans = chunker.chunk(text, tokenizer, n_sentences=1)

        # Extract actual text chunks using char spans
        chunks = [text[start:end] for start, end in char_spans]

        # Should properly identify sentence boundaries despite abbreviations
        assert len(chunks) >= 1
        assert len(char_spans) >= 1

    def test_span_annotations_validity(self, tokenizer, chunker):
        """Test that span annotations are valid token indices"""
        text = "First sentence. Second sentence. Third sentence."
        token_spans, char_spans = chunker.chunk(text, tokenizer, n_sentences=1)

        # Tokenize the input to verify span validity
        inputs = tokenizer(text, return_tensors="pt")
        token_count = inputs["input_ids"].shape[1]

        for start_idx, end_idx in token_spans:
            assert 0 <= start_idx <= token_count
            assert 0 <= end_idx <= token_count
            assert start_idx < end_idx

    def test_chunk_content_matches_spans(self, tokenizer, chunker):
        """Test that chunks correspond to their span annotations"""
        text = "First sentence. Second sentence."
        token_spans, char_spans = chunker.chunk(text, tokenizer, n_sentences=1)

        # Extract actual text chunks using char spans
        chunks = [text[start:end] for start, end in char_spans]

        # Verify that the number of chunks matches spans
        assert len(chunks) == len(char_spans)

        # Each chunk should be non-empty
        for chunk in chunks:
            assert len(chunk.strip()) > 0

    def test_alice_example(self, tokenizer, chunker):
        """Test with the Alice in Wonderland example from the API test"""
        text = """Alice was beginning to get very tired of sitting by her sister on the bank, and of having nothing to do: once or twice she had peeped into the book her sister was reading, but it had no pictures or conversations in it. 'And what is the use of a book,' thought Alice 'without pictures or conversation?' So she was considering in her own mind (as well as she could, for the hot day made her feel very sleepy and stupid), whether the pleasure of making a daisy-chain would be worth the trouble of getting up and picking the daisies, when suddenly a White Rabbit with pink eyes ran close by her."""

        token_spans, char_spans = chunker.chunk(text, tokenizer, n_sentences=1)

        # Extract actual text chunks using char spans
        chunks = [text[start:end] for start, end in char_spans]

        # Should identify multiple sentences
        assert len(chunks) >= 2
        assert len(char_spans) >= 2
        assert len(chunks) == len(char_spans)

        # Verify that chunks contain expected content
        full_text = "".join(chunks)
        # Remove extra whitespace for comparison
        assert full_text.replace(" ", "") == text.replace(" ", "")

    def test_mixed_punctuation(self, tokenizer, chunker):
        """Test sentences with different punctuation marks"""
        text = "Is this working? Yes, it is! What about this. And this?"
        token_spans, char_spans = chunker.chunk(text, tokenizer, n_sentences=1)

        # Extract actual text chunks using char spans
        chunks = [text[start:end] for start, end in char_spans]

        assert len(chunks) == 4
        assert len(char_spans) == 4
        assert "Is this working?" in chunks[0]
        assert "Yes, it is!" in chunks[1]
        assert "What about this." in chunks[2]
        assert "And this?" in chunks[3]


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
