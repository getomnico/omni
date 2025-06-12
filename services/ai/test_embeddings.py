#!/usr/bin/env python3
"""
Test script for the embeddings API
"""
import requests
import json


def test_embeddings_api():
    """Test the embeddings endpoint with both chunking modes"""

    # Test data - same as in the provided example but with multiple sentences
    test_text = """Alice was beginning to get very tired of sitting by her sister on the bank, and of having nothing to do: once or twice she had peeped into the book her sister was reading, but it had no pictures or conversations in it. 'And what is the use of a book,' thought Alice 'without pictures or conversation?' So she was considering in her own mind (as well as she could, for the hot day made her feel very sleepy and stupid), whether the pleasure of making a daisy-chain would be worth the trouble of getting up and picking the daisies, when suddenly a White Rabbit with pink eyes ran close by her."""

    url = "http://localhost:8000/embeddings"

    # Test sentence-based chunking
    print("Testing sentence-based chunking...")
    payload_sentence = {
        "texts": [test_text],
        "task": "retrieval.passage",
        "chunking_mode": "sentence",
    }

    try:
        response = requests.post(url, json=payload_sentence)

        if response.status_code == 200:
            result = response.json()
            print(
                f"Sentence chunking success! Generated embeddings for {len(result['embeddings'])} texts"
            )
            print(f"Sentence chunks count: {result['chunks_count']}")
            print(
                f"Embedding dimensions: {len(result['embeddings'][0]) if result['embeddings'] else 0}"
            )
        else:
            print(f"Sentence chunking error: {response.status_code}")
            print(f"Response: {response.text}")

    except requests.exceptions.ConnectionError:
        print("Connection error - make sure the AI service is running on port 8000")
    except Exception as e:
        print(f"Sentence chunking error: {str(e)}")

    print()

    # Test fixed-size chunking
    print("Testing fixed-size chunking...")
    payload_fixed = {
        "texts": [test_text],
        "task": "retrieval.passage",
        "chunking_mode": "fixed",
        "chunk_size": 512,
    }

    try:
        response = requests.post(url, json=payload_fixed)

        if response.status_code == 200:
            result = response.json()
            print(
                f"Fixed chunking success! Generated embeddings for {len(result['embeddings'])} texts"
            )
            print(f"Fixed chunks count: {result['chunks_count']}")
            print(
                f"Embedding dimensions: {len(result['embeddings'][0]) if result['embeddings'] else 0}"
            )
        else:
            print(f"Fixed chunking error: {response.status_code}")
            print(f"Response: {response.text}")

    except requests.exceptions.ConnectionError:
        print("Connection error - make sure the AI service is running on port 8000")
    except Exception as e:
        print(f"Fixed chunking error: {str(e)}")


def test_health_endpoint():
    """Test the health endpoint"""
    try:
        response = requests.get("http://localhost:8000/health")
        if response.status_code == 200:
            result = response.json()
            print(f"Health check passed: {result}")
        else:
            print(f"Health check failed: {response.status_code}")
    except Exception as e:
        print(f"Health check error: {str(e)}")


if __name__ == "__main__":
    print("Testing Clio AI Service Embeddings API")
    print("=" * 50)

    test_health_endpoint()
    print()
    print("Comparing sentence-based vs fixed-size chunking:")
    print("-" * 50)
    test_embeddings_api()

    print("\nKey differences:")
    print(
        "• Sentence chunking: Respects natural sentence boundaries for more semantic coherence"
    )
    print("• Fixed chunking: Uses fixed token windows, may split sentences mid-way")
    print(
        "• Sentence chunking typically produces more variable chunk counts based on text structure"
    )
