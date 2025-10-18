#!/usr/bin/env python3
"""
Test script for JINA embeddings integration.
Run this script to verify JINA API integration is working correctly.
"""

import asyncio
import os
import sys
from typing import List

# Add parent directory to path to import modules
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from services.ai.embeddings.jina import (
    generate_embeddings_with_jina,
    generate_embeddings_sync,
    JINAEmbeddingClient,
    DEFAULT_TASK,
    QUERY_TASK,
)


async def test_basic_embedding():
    """Test basic embedding generation"""
    print("\n=== Testing Basic Embedding ===")
    
    client = JINAEmbeddingClient()
    
    try:
        texts = [
            "The quick brown fox jumps over the lazy dog.",
            "Machine learning is a subset of artificial intelligence."
        ]
        
        embeddings = await client.generate_embeddings(texts, task=DEFAULT_TASK)
        
        print(f"Generated {len(embeddings)} embeddings")
        for i, emb in enumerate(embeddings):
            print(f"Text {i+1}: embedding dimension = {len(emb)}, first 5 values = {emb[:5]}")
            
        return True
        
    except Exception as e:
        print(f"Error: {e}")
        return False
        
    finally:
        await client.close()


async def test_chunking_modes():
    """Test different chunking modes"""
    print("\n=== Testing Chunking Modes ===")
    
    test_text = [
        "This is the first sentence. This is the second sentence. "
        "This is the third sentence. This is the fourth sentence. "
        "This is the fifth sentence. This is the sixth sentence."
    ]
    
    modes = ["none", "sentence", "fixed"]
    
    for mode in modes:
        print(f"\nTesting chunking mode: {mode}")
        
        try:
            result = await generate_embeddings_with_jina(
                texts=test_text,
                task=DEFAULT_TASK,
                chunk_size=100,
                chunking_mode=mode,
                n_sentences=2 if mode == "sentence" else None
            )
            
            print(f"  Generated {len(result[0])} chunks")
            for i, chunk in enumerate(result[0]):
                print(f"    Chunk {i+1}: span={chunk.span}, embedding_dim={len(chunk.embedding)}")
                
        except Exception as e:
            print(f"  Error: {e}")
            return False
            
    return True


async def test_task_types():
    """Test different task types"""
    print("\n=== Testing Task Types ===")
    
    client = JINAEmbeddingClient()
    
    try:
        test_text = ["Information retrieval is important for search engines."]
        
        # Test query task
        print("Testing retrieval.query task:")
        query_emb = await client.generate_embeddings(test_text, task=QUERY_TASK)
        print(f"  Query embedding dimension: {len(query_emb[0])}")
        
        # Test passage task
        print("Testing retrieval.passage task:")
        passage_emb = await client.generate_embeddings(test_text, task=DEFAULT_TASK)
        print(f"  Passage embedding dimension: {len(passage_emb[0])}")
        
        return True
        
    except Exception as e:
        print(f"Error: {e}")
        return False
        
    finally:
        await client.close()


def test_sync_wrapper():
    """Test synchronous wrapper function"""
    print("\n=== Testing Synchronous Wrapper ===")
    
    try:
        texts = ["Synchronous test text."]
        
        result = generate_embeddings_sync(
            texts=texts,
            task=DEFAULT_TASK,
            chunk_size=512,
            chunking_mode="none",
            n_sentences=None
        )
        
        print(f"Generated {len(result)} text results")
        print(f"First text has {len(result[0])} chunks")
        if result[0]:
            print(f"First chunk: span={result[0][0].span}, embedding_dim={len(result[0][0].embedding)}")
            
        return True
        
    except Exception as e:
        print(f"Error: {e}")
        return False


async def main():
    """Run all tests"""
    print("=" * 50)
    print("JINA Embeddings Integration Test")
    print("=" * 50)
    
    # Check if API key is set
    api_key = os.getenv("JINA_API_KEY", "")
    if not api_key or api_key == "your-jina-api-key-here":
        print("\n⚠️  WARNING: JINA_API_KEY is not set or is still the placeholder value!")
        print("Please set the JINA_API_KEY environment variable to run these tests.")
        print("You can get an API key from: https://jina.ai/embeddings/")
        return
        
    print(f"\n✓ JINA_API_KEY is set")
    print(f"✓ Using model: {os.getenv('JINA_MODEL', 'jina-embeddings-v3')}")
    
    # Run tests
    tests = [
        ("Basic Embedding", test_basic_embedding),
        ("Chunking Modes", test_chunking_modes),
        ("Task Types", test_task_types),
    ]
    
    results = []
    for test_name, test_func in tests:
        if asyncio.iscoroutinefunction(test_func):
            result = await test_func()
        else:
            result = test_func()
        results.append((test_name, result))
        
    # Also test sync wrapper
    sync_result = test_sync_wrapper()
    results.append(("Synchronous Wrapper", sync_result))
    
    # Print summary
    print("\n" + "=" * 50)
    print("Test Summary:")
    print("=" * 50)
    
    for test_name, result in results:
        status = "✓ PASSED" if result else "✗ FAILED"
        print(f"{test_name}: {status}")
        
    all_passed = all(r[1] for r in results)
    if all_passed:
        print("\n🎉 All tests passed!")
    else:
        print("\n❌ Some tests failed. Please check the output above.")


if __name__ == "__main__":
    asyncio.run(main())