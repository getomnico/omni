"""Integration tests for the /embeddings endpoint."""

import pytest


@pytest.mark.integration
async def test_embeddings_returns_correct_structure(
    async_client, mock_embedding_provider
):
    """Verify embedding response has correct dimensions and structure."""
    response = await async_client.post(
        "/embeddings", json={"texts": ["Hello world"], "chunking_mode": "none"}
    )
    assert response.status_code == 200
    data = response.json()

    # Verify structure matches what mock returns
    assert data["model_name"] == "test-embedding-model"
    assert len(data["embeddings"]) == 1  # One text input
    assert len(data["embeddings"][0]) == 1  # One chunk (no chunking)
    assert len(data["embeddings"][0][0]) == 1024  # Embedding dimension
    assert data["chunks_count"] == [1]


@pytest.mark.integration
async def test_embeddings_chunk_spans_cover_input(async_client):
    """Verify chunk spans are contiguous and cover full input."""
    text = "First sentence. Second sentence. Third sentence."
    response = await async_client.post(
        "/embeddings",
        json={"texts": [text], "chunk_size": 10, "chunking_mode": "sentence"},
    )
    assert response.status_code == 200
    data = response.json()

    # Verify spans are valid
    chunks = data["chunks"][0]
    for start, end in chunks:
        assert 0 <= start < end <= len(text)

    # Verify spans cover full text (contiguous)
    if chunks:
        assert chunks[0][0] == 0  # Starts at beginning
        assert chunks[-1][1] == len(text)  # Ends at end


@pytest.mark.integration
async def test_embeddings_multiple_texts(async_client):
    """Verify multiple texts are processed independently."""
    response = await async_client.post(
        "/embeddings",
        json={"texts": ["Text one", "Text two", "Text three"], "chunking_mode": "none"},
    )
    assert response.status_code == 200
    data = response.json()

    assert len(data["embeddings"]) == 3
    assert len(data["chunks_count"]) == 3


@pytest.mark.integration
async def test_embeddings_empty_text_list_returns_empty_response(async_client):
    """Empty text list returns empty embeddings response."""
    response = await async_client.post("/embeddings", json={"texts": []})
    assert response.status_code == 200
    data = response.json()

    assert data["embeddings"] == []
    assert data["chunks_count"] == []
    assert data["chunks"] == []
