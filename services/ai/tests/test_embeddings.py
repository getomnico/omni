#!/usr/bin/env python3
"""
Test suite for the embeddings API using pytest

Usage:
    # Run all tests
    pytest test_embeddings_pytest.py -v

    # Run specific test
    pytest test_embeddings_pytest.py::test_semantic_chunking_detailed -v

    # Run with existing server
    AI_SERVICE_URL=http://localhost:8000 pytest test_embeddings_pytest.py -v
"""
import pytest
import requests
import numpy as np
import os
import subprocess
import time
import sys
import signal
from typing import Generator, Dict, Any


@pytest.fixture(scope="session")
def ai_service_url() -> Generator[str, None, None]:
    """Get AI service URL from environment or start a test server"""
    # Check if we should use an existing service
    existing_url = os.environ.get("AI_SERVICE_URL")
    if existing_url:
        # Verify the service is running
        try:
            response = requests.get(f"{existing_url}/health", timeout=2)
            if response.status_code == 200:
                yield existing_url
                return
        except:
            pytest.fail(f"AI service at {existing_url} is not responding")

    # Start a test server
    port = 8000
    ai_service_dir = os.path.join(os.path.dirname(__file__), "..")
    original_dir = os.getcwd()
    os.chdir(ai_service_dir)

    # Start the server
    server_process = subprocess.Popen(
        [
            sys.executable,
            "-m",
            "uvicorn",
            "main:app",
            "--host",
            "0.0.0.0",
            "--port",
            str(port),
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        preexec_fn=os.setsid if sys.platform != "win32" else None,
    )

    # Wait for server to be ready
    base_url = f"http://localhost:{port}"
    health_url = f"{base_url}/health"
    for i in range(30):
        try:
            response = requests.get(health_url, timeout=1)
            if response.status_code == 200:
                break
        except:
            pass
        time.sleep(1)
    else:
        # Clean up if server didn't start
        if sys.platform != "win32":
            os.killpg(os.getpgid(server_process.pid), signal.SIGTERM)
        else:
            server_process.terminate()
        server_process.wait()
        os.chdir(original_dir)
        pytest.fail("AI service failed to start after 30 seconds")

    yield base_url

    # Cleanup
    if sys.platform != "win32":
        os.killpg(os.getpgid(server_process.pid), signal.SIGTERM)
    else:
        server_process.terminate()

    try:
        server_process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        if sys.platform != "win32":
            os.killpg(os.getpgid(server_process.pid), signal.SIGKILL)
        else:
            server_process.kill()
        server_process.wait()

    os.chdir(original_dir)


@pytest.fixture
def embeddings_url(ai_service_url: str) -> str:
    """Get the embeddings endpoint URL"""
    return f"{ai_service_url}/embeddings"


@pytest.fixture
def multi_paragraph_document() -> str:
    """A document with clear semantic boundaries for testing semantic chunking"""
    return """Machine learning has revolutionized the way we approach data analysis and pattern recognition. Traditional statistical methods often require manual feature engineering and domain expertise to identify relevant patterns in data. Modern machine learning algorithms can automatically discover complex relationships and representations from raw data, making them particularly powerful for tasks like image recognition, natural language processing, and predictive analytics.

The field of artificial intelligence encompasses much more than just machine learning. AI includes symbolic reasoning, expert systems, knowledge representation, and planning algorithms that don't necessarily rely on learning from data. Classical AI systems were built using rule-based approaches where human experts would encode their knowledge into formal logical systems. These systems could perform complex reasoning tasks but lacked the flexibility to adapt to new situations or learn from experience.

Cloud computing has fundamentally changed how organizations deploy and scale their software applications. Instead of maintaining expensive on-premises hardware, companies can now leverage elastic computing resources that scale automatically based on demand. This shift has enabled startups to compete with large enterprises by accessing enterprise-grade infrastructure without the massive upfront capital investment. Popular cloud platforms like AWS, Google Cloud, and Microsoft Azure provide comprehensive services ranging from basic compute and storage to advanced AI and machine learning capabilities.

Database management systems form the backbone of most modern applications, storing and retrieving vast amounts of structured and unstructured data. Relational databases like PostgreSQL and MySQL have dominated the landscape for decades, providing ACID guarantees and SQL query capabilities. However, the rise of NoSQL databases like MongoDB, Cassandra, and Redis has provided developers with more flexible data models and horizontal scaling capabilities for specific use cases like real-time analytics, content management, and session storage."""


def test_health_endpoint(ai_service_url: str):
    """Test the health endpoint"""
    response = requests.get(f"{ai_service_url}/health")
    assert response.status_code == 200

    result = response.json()
    assert "status" in result
    assert result["status"] == "healthy"


class TestSemanticChunking:
    """Test suite for semantic chunking functionality"""

    def test_semantic_chunking_detailed(
        self, embeddings_url: str, multi_paragraph_document: str
    ):
        """Test semantic chunking with a multi-paragraph document"""
        payload = {
            "texts": [multi_paragraph_document],
            "task": "retrieval.passage",
            "chunking_mode": "semantic",
        }

        response = requests.post(embeddings_url, json=payload)
        assert response.status_code == 200

        result = response.json()

        # Verify response structure
        assert "embeddings" in result
        assert "chunks_count" in result
        assert "chunks" in result

        # Get the chunk spans and embeddings
        chunk_spans = result["chunks"][0]
        chunk_embeddings = result["embeddings"][0]

        # Verify we got multiple chunks (semantic chunking should create fewer chunks than sentences)
        print(f"Embeddings API returned {len(chunk_spans)} chunks")
        assert (
            len(chunk_spans) > 1
        ), f"Expected multiple semantic chunks, got {len(chunk_spans)}"

        # Verify each chunk span is valid
        for i, (start, end) in enumerate(chunk_spans):
            assert start < end, f"Chunk {i} has invalid span: ({start}, {end})"
            assert start >= 0, f"Chunk {i} has negative start: {start}"

        # Verify embeddings match chunks
        assert len(chunk_embeddings) == len(chunk_spans)

        # Verify embedding properties
        for i, embedding in enumerate(chunk_embeddings):
            assert len(embedding) == 1024  # jina-embeddings-v3 embedding dimensions
            norm = np.linalg.norm(embedding)
            assert 0.99 < norm < 1.01, f"Embedding {i} should be normalized"

    def test_semantic_vs_sentence_chunking(
        self, embeddings_url: str, multi_paragraph_document: str
    ):
        """Compare semantic chunking with sentence chunking"""
        # Get semantic chunks
        semantic_response = requests.post(
            embeddings_url,
            json={
                "texts": [multi_paragraph_document],
                "task": "retrieval.passage",
                "chunking_mode": "semantic",
            },
        )
        assert semantic_response.status_code == 200
        semantic_chunk_spans = semantic_response.json()["chunks"][0]

        # Get sentence chunks
        sentence_response = requests.post(
            embeddings_url,
            json={
                "texts": [multi_paragraph_document],
                "task": "retrieval.passage",
                "chunking_mode": "sentence",
            },
        )
        assert sentence_response.status_code == 200
        sentence_chunk_spans = sentence_response.json()["chunks"][0]

        # Semantic chunking should produce fewer chunks than sentence chunking
        assert len(semantic_chunk_spans) < len(
            sentence_chunk_spans
        ), f"Semantic chunks ({len(semantic_chunk_spans)}) should be fewer than sentence chunks ({len(sentence_chunk_spans)})"

    def test_semantic_chunking_span_coverage(
        self, embeddings_url: str, multi_paragraph_document: str
    ):
        """Ensure semantic chunking spans are non-overlapping and well-formed"""
        response = requests.post(
            embeddings_url,
            json={
                "texts": [multi_paragraph_document],
                "task": "retrieval.passage",
                "chunking_mode": "semantic",
            },
        )
        assert response.status_code == 200

        chunk_spans = response.json()["chunks"][0]

        # Verify spans are non-overlapping and in order
        for i in range(len(chunk_spans) - 1):
            current_start, current_end = chunk_spans[i]
            next_start, next_end = chunk_spans[i + 1]

            # Current span should be valid
            assert (
                current_start < current_end
            ), f"Invalid span at index {i}: ({current_start}, {current_end})"

            # Next span should start after current span ends (non-overlapping)
            assert (
                current_end <= next_start
            ), f"Overlapping spans at index {i} and {i+1}: ({current_start}, {current_end}) and ({next_start}, {next_end})"


class TestChunkingModes:
    """Test suite for different chunking modes"""

    @pytest.mark.parametrize(
        "mode,expected_chunks",
        [
            ("sentence", 3),  # Will be determined by sentence count
            ("fixed", 4),  # Will be determined by text length
            ("semantic", 2),  # Will be determined by semantic boundaries
        ],
    )
    def test_chunking_mode(self, embeddings_url: str, mode: str, expected_chunks: int):
        """Test different chunking modes"""
        test_text = (
            "This is a test. It has multiple sentences. Each sentence is separate."
        )

        payload = {
            "texts": [test_text],
            "task": "retrieval.passage",
            "chunking_mode": mode,
        }

        if mode == "fixed":
            payload["chunk_size"] = 4

        response = requests.post(embeddings_url, json=payload)
        assert response.status_code == 200

        result = response.json()
        embeddings = result["embeddings"][0]
        chunk_spans = result["chunks"][0]

        assert len(embeddings) > 0
        assert all(len(emb) == 1024 for emb in embeddings)

        assert len(chunk_spans) == expected_chunks


class TestEdgeCases:
    """Test suite for edge cases and error handling"""

    def test_empty_text(self, embeddings_url: str):
        """Test handling of empty text"""
        response = requests.post(
            embeddings_url,
            json={
                "texts": [""],
                "task": "retrieval.passage",
                "chunking_mode": "sentence",
            },
        )
        assert response.status_code == 200
        result = response.json()
        assert len(result["embeddings"][0]) == 0

    def test_multiple_texts(self, embeddings_url: str):
        """Test handling of multiple texts"""
        texts = ["First text.", "Second text.", "Third text."]
        response = requests.post(
            embeddings_url,
            json={
                "texts": texts,
                "task": "retrieval.passage",
                "chunking_mode": "sentence",
            },
        )
        assert response.status_code == 200
        result = response.json()
        assert len(result["embeddings"]) == 3
        assert all(len(chunks) > 0 for chunks in result["embeddings"])

    def test_invalid_chunking_mode(self, embeddings_url: str):
        """Test rejection of invalid chunking mode"""
        response = requests.post(
            embeddings_url,
            json={
                "texts": ["Test text"],
                "task": "retrieval.passage",
                "chunking_mode": "invalid",
            },
        )
        assert response.status_code == 422

    def test_missing_required_fields(self, embeddings_url: str):
        """Test rejection of request with missing fields"""
        response = requests.post(embeddings_url, json={})
        assert response.status_code == 422


class TestEmbeddingQuality:
    """Test suite for embedding quality checks"""

    def test_embedding_normalization(self, embeddings_url: str):
        """Test that all embeddings are normalized"""
        response = requests.post(
            embeddings_url,
            json={
                "texts": ["Test text for normalization check."],
                "task": "retrieval.passage",
                "chunking_mode": "sentence",
            },
        )
        assert response.status_code == 200

        embeddings = response.json()["embeddings"][0]
        for embedding in embeddings:
            norm = np.linalg.norm(embedding)
            assert 0.99 < norm < 1.01

    def test_embedding_similarity(self, embeddings_url: str):
        """Test that similar texts produce similar embeddings"""
        similar_texts = [
            "Machine learning is a subset of artificial intelligence.",
            "Machine learning is part of the field of artificial intelligence.",
        ]

        response = requests.post(
            embeddings_url,
            json={
                "texts": similar_texts,
                "task": "retrieval.passage",
                "chunking_mode": "sentence",
            },
        )
        assert response.status_code == 200

        embeddings = response.json()["embeddings"]
        emb1 = np.array(embeddings[0][0])
        emb2 = np.array(embeddings[1][0])

        # Cosine similarity should be high for similar texts
        cosine_sim = np.dot(emb1, emb2) / (np.linalg.norm(emb1) * np.linalg.norm(emb2))
        assert (
            cosine_sim > 0.8
        ), f"Similar texts should have high cosine similarity, got {cosine_sim}"
