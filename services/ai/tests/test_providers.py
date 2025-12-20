#!/usr/bin/env python3
"""
Test script for LLM providers to verify streaming functionality.
"""

import asyncio
import os
import sys
import json
from typing import Optional

# Add the current directory to the Python path
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from providers import create_llm_provider, LLMProvider


async def test_provider_streaming(provider: LLMProvider, provider_name: str):
    """Test streaming functionality of a provider."""
    print(f"\n=== Testing {provider_name} Provider ===")

    # Test health check
    try:
        health = await provider.health_check()
        print(f"Health check: {'✓ PASS' if health else '✗ FAIL'}")
    except Exception as e:
        print(f"Health check: ✗ ERROR - {e}")
        return False

    # Test streaming response
    try:
        print("Testing streaming response...")
        prompt = "What is the capital of France? Please answer in one sentence."

        response_chunks = []
        async for chunk in provider.stream_response(
            prompt=prompt, max_tokens=100, temperature=0.7
        ):
            response_chunks.append(chunk)
            print(f"Chunk: {chunk}", end="", flush=True)

        full_response = "".join(response_chunks)
        print(f"\nFull response: {full_response}")
        print(f"Streaming: {'✓ PASS' if full_response else '✗ FAIL'}")

    except Exception as e:
        print(f"Streaming: ✗ ERROR - {e}")
        return False

    # Test non-streaming response
    try:
        print("Testing non-streaming response...")
        response = await provider.generate_response(
            prompt="What is 2+2? Answer with just the number.",
            max_tokens=10,
            temperature=0.1,
        )
        print(f"Non-streaming response: {response}")
        print(f"Non-streaming: {'✓ PASS' if response else '✗ FAIL'}")

    except Exception as e:
        print(f"Non-streaming: ✗ ERROR - {e}")
        return False

    return True


async def main():
    """Main test function."""
    print("LLM Provider Testing Suite")
    print("=" * 50)

    # Test configuration
    test_vllm = os.getenv("TEST_VLLM", "false").lower() == "true"
    test_anthropic = os.getenv("TEST_ANTHROPIC", "false").lower() == "true"
    test_bedrock = os.getenv("TEST_BEDROCK", "false").lower() == "true"

    vllm_url = os.getenv("VLLM_URL", "http://localhost:8000")
    anthropic_api_key = os.getenv("ANTHROPIC_API_KEY", "")
    anthropic_model = os.getenv("ANTHROPIC_MODEL", "claude-3-5-sonnet-20241022")
    bedrock_model_id = os.getenv(
        "BEDROCK_MODEL_ID", "us.anthropic.claude-sonnet-4-20250514-v1:0"
    )
    aws_region = os.getenv("AWS_REGION", "")

    results = []

    # Test vLLM provider
    if test_vllm:
        if not vllm_url:
            print("❌ VLLM_URL not set, skipping vLLM test")
        else:
            try:
                vllm_provider = create_llm_provider("vllm", vllm_url=vllm_url)
                result = await test_provider_streaming(vllm_provider, "vLLM")
                results.append(("vLLM", result))
            except Exception as e:
                print(f"❌ Failed to create vLLM provider: {e}")
                results.append(("vLLM", False))

    # Test Anthropic provider
    if test_anthropic:
        if not anthropic_api_key:
            print("❌ ANTHROPIC_API_KEY not set, skipping Anthropic test")
        else:
            try:
                anthropic_provider = create_llm_provider(
                    "anthropic", api_key=anthropic_api_key, model=anthropic_model
                )
                result = await test_provider_streaming(anthropic_provider, "Anthropic")
                results.append(("Anthropic", result))
            except Exception as e:
                print(f"❌ Failed to create Anthropic provider: {e}")
                results.append(("Anthropic", False))

    # Test Bedrock provider
    if test_bedrock:
        try:
            region_name = aws_region if aws_region else None
            bedrock_provider = create_llm_provider(
                "bedrock", model_id=bedrock_model_id, region_name=region_name
            )
            result = await test_provider_streaming(bedrock_provider, "Bedrock")
            results.append(("Bedrock", result))
        except Exception as e:
            print(f"❌ Failed to create Bedrock provider: {e}")
            results.append(("Bedrock", False))

    # Print summary
    print("\n" + "=" * 50)
    print("TEST SUMMARY")
    print("=" * 50)

    for provider_name, result in results:
        status = "✓ PASS" if result else "✗ FAIL"
        print(f"{provider_name}: {status}")

    if not results:
        print(
            "No tests were run. Set TEST_VLLM=true, TEST_ANTHROPIC=true, or TEST_BEDROCK=true to run tests."
        )
        print("\nExample usage:")
        print("  # Test vLLM provider")
        print(
            "  TEST_VLLM=true VLLM_URL=http://localhost:8000 python test_providers.py"
        )
        print("\n  # Test Anthropic provider")
        print(
            "  TEST_ANTHROPIC=true ANTHROPIC_API_KEY=your-key python test_providers.py"
        )
        print("\n  # Test Bedrock provider")
        print("  TEST_BEDROCK=true python test_providers.py")
        print("\n  # Test all providers")
        print(
            "  TEST_VLLM=true TEST_ANTHROPIC=true TEST_BEDROCK=true VLLM_URL=http://localhost:8000 ANTHROPIC_API_KEY=your-key python test_providers.py"
        )

    return all(result for _, result in results) if results else False


if __name__ == "__main__":
    success = asyncio.run(main())
    sys.exit(0 if success else 1)
