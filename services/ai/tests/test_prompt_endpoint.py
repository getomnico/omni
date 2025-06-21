#!/usr/bin/env python3
"""
Test script for the new /prompt endpoint in clio-ai service
"""
import httpx
import asyncio
import json
import sys
import os


async def test_prompt_endpoint():
    """Test the /prompt endpoint"""

    # Test payload
    test_payload = {
        "prompt": "Hello, how are you today?",
        "max_tokens": 100,
        "temperature": 0.7,
        "top_p": 0.9,
    }

    # AI service URL from environment or default
    ai_service_url = os.environ.get("AI_SERVICE_URL", "http://localhost:3003")

    try:
        async with httpx.AsyncClient(timeout=30.0) as client:
            print(f"Testing /prompt endpoint at {ai_service_url}")
            print(f"Payload: {json.dumps(test_payload, indent=2)}")

            response = await client.post(f"{ai_service_url}/prompt", json=test_payload)

            print(f"Response status: {response.status_code}")
            print(f"Response headers: {dict(response.headers)}")

            if response.status_code == 200:
                response_data = response.json()
                print(f"Response data: {json.dumps(response_data, indent=2)}")
                print("✅ Test passed!")
            else:
                print(f"❌ Test failed with status {response.status_code}")
                print(f"Error response: {response.text}")

    except Exception as e:
        print(f"❌ Test failed with exception: {str(e)}")
        sys.exit(1)


async def test_health_endpoint():
    """Test the health endpoint first"""
    ai_service_url = os.environ.get("AI_SERVICE_URL", "http://localhost:3003")

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
    print("Starting clio-ai service tests...")

    # Test health endpoint first
    health_ok = await test_health_endpoint()
    if not health_ok:
        print("⚠️  Health check failed, service may not be running")
        print("Make sure the clio-ai service is running on port 3003")
        return

    print("\n" + "=" * 50)

    # Test the new prompt endpoint
    await test_prompt_endpoint()


if __name__ == "__main__":
    asyncio.run(main())
