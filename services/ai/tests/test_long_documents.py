#!/usr/bin/env python3
"""
Test handling of long documents that exceed the token limit
"""
import pytest
from transformers import AutoTokenizer
from embeddings import (
    generate_embeddings_sync,
    process_single_text,
    split_text_into_segments,
    MAX_LENGTH,
    load_model,
    get_chunker,
)


class TestLongDocuments:
    """Test cases for handling long documents"""

    @pytest.fixture
    def tokenizer(self):
        """Load the tokenizer for testing"""
        return AutoTokenizer.from_pretrained(
            "jinaai/jina-embeddings-v3", trust_remote_code=True
        )

    @pytest.fixture
    def model(self):
        """Load the model for testing"""
        model, _ = load_model()
        return model

    def test_split_text_into_segments(self, tokenizer):
        """Test splitting long text into segments"""
        # Create a long text that exceeds MAX_LENGTH tokens
        # Use varied sentences to ensure token count is high
        varied_sentences = [
            "This is the first test sentence with unique content and vocabulary.",
            "The second sentence contains different words and expressions entirely.",
            "A third sentence with completely distinct terminology and phrasing.",
            "Fourth sentence uses alternative language constructs and semantic elements.",
            "The fifth sentence employs various lexical items and grammatical structures.",
        ]

        # Repeat the varied sentences to create a long text
        long_text = " ".join(varied_sentences * 400)  # Should exceed 8192 tokens

        # Verify the text actually exceeds the limit
        test_tokens = tokenizer(long_text, return_tensors="pt", truncation=False)
        actual_token_count = test_tokens["input_ids"].shape[1]
        print(f"Actual token count: {actual_token_count}")

        if actual_token_count <= MAX_LENGTH:
            # Make it even longer if needed
            long_text = " ".join(varied_sentences * 800)
            test_tokens = tokenizer(long_text, return_tensors="pt", truncation=False)
            actual_token_count = test_tokens["input_ids"].shape[1]
            print(f"Extended token count: {actual_token_count}")

        segments = split_text_into_segments(long_text, tokenizer, MAX_LENGTH)

        if actual_token_count > MAX_LENGTH:
            assert (
                len(segments) > 1
            ), f"Long text with {actual_token_count} tokens should be split into multiple segments, got {len(segments)}"

        # Verify each segment fits within MAX_LENGTH
        for i, segment in enumerate(segments):
            tokens = tokenizer(segment, return_tensors="pt", truncation=False)
            token_count = tokens["input_ids"].shape[1]
            assert (
                token_count <= MAX_LENGTH
            ), f"Segment {i} has {token_count} tokens, exceeds {MAX_LENGTH}"

    def test_process_long_text_with_semantic_chunking(self, model, tokenizer):
        """Test processing a long document with semantic chunking"""
        # Create a long document
        long_text = (
            """
        The history of artificial intelligence (AI) began in antiquity, with myths, stories and rumors of artificial beings endowed with intelligence or consciousness by master craftsmen. The seeds of modern AI were planted by philosophers who attempted to describe the process of human thinking as the mechanical manipulation of symbols. This work culminated in the invention of the programmable digital computer in the 1940s, a machine based on the abstract essence of mathematical reasoning. This device and the ideas behind it inspired a handful of scientists to begin seriously discussing the possibility of building an electronic brain.
        
        The field of AI research was founded at a workshop held on the campus of Dartmouth College, USA during the summer of 1956. Those who attended would become the leaders of AI research for decades. Many of them predicted that a machine as intelligent as a human being would exist in no more than a generation, and they were given millions of dollars to make this vision come true. Eventually, it became obvious that commercial developers and researchers had grossly underestimated the difficulty of the project. In 1974, in response to the criticism from James Lighthill and ongoing pressure from congress, the U.S. and British governments stopped funding undirected research into artificial intelligence.
        
        Seven years later a visionary initiative by the Japanese Government inspired governments and industry to provide AI with billions of dollars, but by the late 1980s the investors became disillusioned and withdrew funding again. This cycle of boom and bust, of "AI winters" and "AI springs", continues to haunt the field. Undaunted, there are those who make extraordinary predictions even now.
        """
            * 10
        )  # Make it really long

        chunker = get_chunker("semantic")
        embeddings, chunks_count, chunk_spans = process_single_text(
            long_text, model, tokenizer, chunker, "retrieval.passage", 512, "semantic"
        )

        assert chunks_count > 0, "Should produce at least one chunk"
        assert (
            len(embeddings) == chunks_count
        ), "Embeddings count should match chunks count"
        assert (
            len(chunk_spans) == chunks_count
        ), "Chunk spans count should match chunks count"

        # Verify all chunk spans are valid
        for i, (start, end) in enumerate(chunk_spans):
            assert start < end, f"Chunk {i} has invalid span: start={start}, end={end}"
            assert start >= 0, f"Chunk {i} start offset is negative: {start}"
            assert end <= len(
                long_text
            ), f"Chunk {i} end offset exceeds text length: {end} > {len(long_text)}"

    def test_edge_cases(self, model, tokenizer):
        """Test edge cases like empty text, single character, etc."""
        chunker = get_chunker("fixed")

        # Test empty text
        embeddings, chunks_count, chunk_spans = process_single_text(
            "", model, tokenizer, chunker, "retrieval.passage", 512, "fixed"
        )
        assert chunks_count == 0, "Empty text should produce no chunks"
        assert len(embeddings) == 0, "Empty text should produce no embeddings"

        # Test single character
        embeddings, chunks_count, chunk_spans = process_single_text(
            "A", model, tokenizer, chunker, "retrieval.passage", 512, "fixed"
        )
        assert chunks_count > 0, "Single character should produce at least one chunk"

        # Test text with only whitespace
        embeddings, chunks_count, chunk_spans = process_single_text(
            "   \n\t  ", model, tokenizer, chunker, "retrieval.passage", 512, "fixed"
        )
        # Should handle gracefully without errors

    def test_generate_embeddings_sync_with_multiple_texts(self):
        """Test the main embedding generation function with multiple texts of varying lengths"""
        texts = [
            "Short text.",
            "Medium length text that contains more information but is still manageable.",
            "This is a much longer text. " * 500,  # Long text
            "",  # Empty text
            "Another short text.",
        ]

        embeddings, chunks_counts, all_chunk_spans = generate_embeddings_sync(
            texts, "retrieval.passage", 512, "fixed"
        )

        assert len(embeddings) == len(texts), "Should return embeddings for all texts"
        assert len(chunks_counts) == len(
            texts
        ), "Should return chunk counts for all texts"
        assert len(all_chunk_spans) == len(
            texts
        ), "Should return chunk spans for all texts"

        # Verify empty text handling
        assert chunks_counts[3] == 0, "Empty text should have 0 chunks"
        assert len(embeddings[3]) == 0, "Empty text should have no embeddings"

        # Verify long text is chunked
        assert chunks_counts[2] > 1, "Long text should be split into multiple chunks"

        # Verify all chunk spans are valid
        for text_idx, (text, chunk_spans) in enumerate(zip(texts, all_chunk_spans)):
            for chunk_idx, (start, end) in enumerate(chunk_spans):
                assert (
                    start < end
                ), f"Text {text_idx}, chunk {chunk_idx} has invalid span: start={start}, end={end}"

    def test_chunk_bounds_validation(self, model, tokenizer):
        """Test that invalid chunk bounds are filtered out"""
        # Create text that might produce edge case chunks
        text = "A. B. C."  # Short sentences that might produce edge cases

        chunker = get_chunker("sentence")
        embeddings, chunks_count, chunk_spans = process_single_text(
            text, model, tokenizer, chunker, "retrieval.passage", 512, "sentence"
        )

        # All returned chunks should have valid bounds
        for start, end in chunk_spans:
            assert start < end, f"Invalid chunk span returned: start={start}, end={end}"


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
