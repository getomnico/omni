#!/usr/bin/env python3
"""
Test script for the new /prompt endpoint in omni-ai service
"""
import httpx
import asyncio
import json
import sys
import os


async def test_prompt_endpoint():
    """Test the /prompt endpoint"""

    # Test payload for non-streaming
    test_payload = {
        "prompt": "What is the capital of France? Answer in one sentence.",
        "max_tokens": 100,
        "temperature": 0.7,
        "top_p": 0.9,
        "stream": False,
    }

    # AI service URL from environment or default
    ai_service_url = os.environ.get("AI_SERVICE_URL", "http://localhost:8001")

    try:
        async with httpx.AsyncClient(timeout=30.0) as client:
            print(f"Testing /prompt endpoint (non-streaming) at {ai_service_url}")
            print(f"Payload: {json.dumps(test_payload, indent=2)}")

            response = await client.post(f"{ai_service_url}/prompt", json=test_payload)

            print(f"Response status: {response.status_code}")
            print(f"Response headers: {dict(response.headers)}")

            if response.status_code == 200:
                response_data = response.json()
                print(f"Response data: {json.dumps(response_data, indent=2)}")
                print("✅ Non-streaming test passed!")
            else:
                print(
                    f"❌ Non-streaming test failed with status {response.status_code}"
                )
                print(f"Error response: {response.text}")

    except Exception as e:
        print(f"❌ Non-streaming test failed with exception: {str(e)}")
        return False

    return True


async def test_prompt_endpoint_streaming():
    """Test the /prompt endpoint with streaming"""

    # Test payload for streaming
    test_payload = {
        "prompt": "Count from 1 to 5, with one number per line.",
        "max_tokens": 50,
        "temperature": 0.1,
        "top_p": 0.9,
        "stream": True,
    }

    # AI service URL from environment or default
    ai_service_url = os.environ.get("AI_SERVICE_URL", "http://localhost:8001")

    try:
        async with httpx.AsyncClient(timeout=30.0) as client:
            print(f"Testing /prompt endpoint (streaming) at {ai_service_url}")
            print(f"Payload: {json.dumps(test_payload, indent=2)}")

            response = await client.post(f"{ai_service_url}/prompt", json=test_payload)

            print(f"Response status: {response.status_code}")
            print(f"Response headers: {dict(response.headers)}")

            if response.status_code == 200:
                print("Streaming response:")
                async for chunk in response.aiter_text():
                    print(chunk, end="", flush=True)
                print("\n✅ Streaming test passed!")
            else:
                print(f"❌ Streaming test failed with status {response.status_code}")
                print(f"Error response: {response.text}")
                return False

    except Exception as e:
        print(f"❌ Streaming test failed with exception: {str(e)}")
        return False

    return True


async def test_health_endpoint():
    """Test the health endpoint first"""
    ai_service_url = os.environ.get("AI_SERVICE_URL", "http://localhost:8001")

    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            print(f"Testing /health endpoint at {ai_service_url}")

            response = await client.get(f"{ai_service_url}/health")

            print(f"Health check status: {response.status_code}")
            if response.status_code == 200:
                health_data = response.json()
                print(f"Health data: {json.dumps(health_data, indent=2)}")
                print("✅ Health check passed!")
                return True
            else:
                print(f"❌ Health check failed with status {response.status_code}")
                return False

    except Exception as e:
        print(f"❌ Health check failed with exception: {str(e)}")
        return False


async def main():
    """Main test function"""
    print("Starting omni-ai service tests...")

    # Test health endpoint first
    health_ok = await test_health_endpoint()
    if not health_ok:
        print("⚠️  Health check failed, service may not be running")
        print("Make sure the omni-ai service is running on port 8001")
        return

    print("\n" + "=" * 50)

    # Test the prompt endpoint (non-streaming)
    non_streaming_ok = await test_prompt_endpoint()

    print("\n" + "=" * 50)

    # Test the prompt endpoint (streaming)
    if non_streaming_ok:
        streaming_ok = await test_prompt_endpoint_streaming()

    print("\n" + "=" * 50)
    print("Test Summary:")
    print(f"Health check: {'✅' if health_ok else '❌'}")
    print(f"Non-streaming prompt: {'✅' if non_streaming_ok else '❌'}")
    if non_streaming_ok:
        print(f"Streaming prompt: {'✅' if streaming_ok else '❌'}")


if __name__ == "__main__":
    asyncio.run(main())
