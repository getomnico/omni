import os
import sys
from fastapi import FastAPI, HTTPException
from fastapi.responses import StreamingResponse
from pydantic import BaseModel
from typing import List, Optional, Tuple
import logging
import asyncio
import httpx
import json
from concurrent.futures import ThreadPoolExecutor

from embeddings_v2 import (
    load_model,
    generate_embeddings_sync,
    DEFAULT_TASK,
)


def get_required_env(key: str) -> str:
    """Get required environment variable with validation"""
    value = os.getenv(key)
    if not value:
        print(
            f"ERROR: Required environment variable '{key}' is not set", file=sys.stderr
        )
        print(
            "Please set this variable in your .env file or environment", file=sys.stderr
        )
        sys.exit(1)
    return value


def get_optional_env(key: str, default: str) -> str:
    """Get optional environment variable with default"""
    return os.getenv(key, default)


def validate_port(port_str: str) -> int:
    """Validate port number"""
    try:
        port = int(port_str)
        if port < 1 or port > 65535:
            raise ValueError("Port must be between 1 and 65535")
        return port
    except ValueError as e:
        print(f"ERROR: Invalid port number '{port_str}': {e}", file=sys.stderr)
        sys.exit(1)


def validate_embedding_dimensions(dims_str: str) -> int:
    """Validate embedding dimensions"""
    try:
        dims = int(dims_str)
        if dims < 1:
            raise ValueError("Embedding dimensions must be positive")
        return dims
    except ValueError as e:
        print(f"ERROR: Invalid embedding dimensions '{dims_str}': {e}", file=sys.stderr)
        sys.exit(1)


# Load and validate configuration
PORT = validate_port(get_required_env("PORT"))
MODEL_PATH = get_required_env("MODEL_PATH")
EMBEDDING_MODEL = get_required_env("EMBEDDING_MODEL")
EMBEDDING_DIMENSIONS = validate_embedding_dimensions(
    get_required_env("EMBEDDING_DIMENSIONS")
)
VLLM_URL = get_required_env("VLLM_URL")
REDIS_URL = get_required_env("REDIS_URL")
DATABASE_URL = get_required_env("DATABASE_URL")

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

app = FastAPI(title="Clio AI Service", version="0.1.0")

# Thread pool for async operations
_executor = ThreadPoolExecutor(max_workers=2)


# Pydantic models
class EmbeddingRequest(BaseModel):
    texts: List[str]
    task: Optional[str] = DEFAULT_TASK  # Allow different tasks
    chunk_size: Optional[int] = (
        512  # Chunk size in tokens for both fixed and sentence modes
    )
    chunking_mode: Optional[str] = (
        "sentence"  # "sentence", "fixed", "semantic", or "none"
    )
    n_sentences: Optional[int] = (
        None  # Number of sentences per chunk (sentence mode only, overrides chunk_size)
    )


class EmbeddingResponse(BaseModel):
    embeddings: List[List[List[float]]]
    chunks_count: List[int]  # Number of chunks per text
    chunks: List[List[Tuple[int, int]]]  # Character offset spans for each chunk
    model_name: str  # Name of the model used for embeddings


class RAGRequest(BaseModel):
    query: str
    documents: List[str]


class RAGResponse(BaseModel):
    answer: str
    relevant_chunks: List[str]


class PromptRequest(BaseModel):
    prompt: str
    max_tokens: Optional[int] = 512
    temperature: Optional[float] = 0.7
    top_p: Optional[float] = 0.9
    stream: Optional[bool] = True


class PromptResponse(BaseModel):
    response: str


@app.on_event("startup")
async def startup_event():
    """Load model on startup"""
    await asyncio.get_event_loop().run_in_executor(_executor, load_model)


@app.get("/health")
async def health_check():
    """Health check endpoint"""
    return {
        "status": "healthy",
        "service": "ai",
        "model": EMBEDDING_MODEL,
        "port": PORT,
        "embedding_dimensions": EMBEDDING_DIMENSIONS,
    }


@app.post("/embeddings", response_model=EmbeddingResponse)
async def generate_embeddings(request: EmbeddingRequest):
    """Generate embeddings for input texts using configurable chunking

    Chunking behavior:
    - fixed mode: chunk_size sets the number of tokens per chunk
    - sentence mode:
      - If n_sentences is provided: groups n_sentences per chunk
      - If only chunk_size is provided: groups sentences until chunk_size tokens limit
    """
    logger.info(
        f"Generating embeddings for {len(request.texts)} texts with chunking_mode={request.chunking_mode}, chunk_size={request.chunk_size}, n_sentences={request.n_sentences}"
    )
    logger.info(f"Input text for generating embeddings: {request.texts}")

    # Validate chunking method
    valid_chunking_modes = ["sentence", "fixed", "semantic", "none"]
    if request.chunking_mode not in valid_chunking_modes:
        raise HTTPException(
            status_code=422,
            detail=f"Invalid chunking_mode: {request.chunking_mode}. Must be one of: {valid_chunking_modes}",
        )

    try:
        # Run embedding generation in thread pool to avoid blocking
        chunk_batch = await asyncio.get_event_loop().run_in_executor(
            _executor,
            generate_embeddings_sync,
            request.texts,
            request.task,
            request.chunk_size,
            request.chunking_mode,
            request.n_sentences,
        )

        logger.info(
            f"Generated these many chunks for each input text: {[len(chunks) for chunks in chunk_batch]}"
        )
        return EmbeddingResponse(
            embeddings=[[c.embedding for c in chunks] for chunks in chunk_batch],
            chunks_count=[len(chunks) for chunks in chunk_batch],
            chunks=[[c.span for c in chunks] for chunks in chunk_batch],
            model_name=EMBEDDING_MODEL,
        )

    except Exception as e:
        logger.error(f"Failed to generate embeddings: {str(e)}")
        raise HTTPException(status_code=500, detail=str(e))


@app.post("/rag", response_model=RAGResponse)
async def rag_inference(request: RAGRequest):
    """Perform RAG inference with retrieved documents"""
    # TODO: Implement RAG pipeline with vLLM integration
    logger.info(f"RAG inference for query: {request.query[:50]}...")

    # Placeholder response
    return RAGResponse(
        answer="This is a placeholder RAG response",
        relevant_chunks=request.documents[:3],  # Return top 3 chunks
    )


@app.post("/prompt")
async def generate_response(request: PromptRequest):
    """Generate a response from the vLLM model for any given prompt with streaming support"""
    logger.info(
        f"Generating response for prompt: {request.prompt[:50]}... (stream={request.stream})"
    )

    if not request.stream:
        # Non-streaming response (keep for backward compatibility)
        return await generate_non_streaming_response(request)

    # Streaming response
    async def stream_generator():
        try:
            # Prepare the request payload for vLLM OpenAI-compatible API
            vllm_payload = {
                "model": "placeholder",  # vLLM ignores this but requires it
                "messages": [{"role": "user", "content": request.prompt}],
                "max_tokens": request.max_tokens,
                "temperature": request.temperature,
                "top_p": request.top_p,
                "stream": True,
            }

            # Make streaming request to vLLM service
            async with httpx.AsyncClient(timeout=60.0) as client:
                async with client.stream(
                    "POST",
                    f"{VLLM_URL}/v1/chat/completions",
                    json=vllm_payload,
                    headers={"Accept": "text/event-stream"},
                ) as response:
                    response.raise_for_status()

                    async for chunk in response.aiter_lines():
                        if chunk:
                            # Skip empty lines and "data: " prefix
                            if chunk.startswith("data: "):
                                chunk_data = chunk[6:]  # Remove "data: " prefix

                                # Skip [DONE] signal
                                if chunk_data == "[DONE]":
                                    break

                                try:
                                    # Parse the JSON chunk
                                    chunk_json = json.loads(chunk_data)

                                    # Extract content from OpenAI format
                                    choices = chunk_json.get("choices", [])
                                    if choices and len(choices) > 0:
                                        delta = choices[0].get("delta", {})
                                        content = delta.get("content", "")
                                        if content:
                                            yield content

                                except json.JSONDecodeError:
                                    # Skip malformed JSON chunks
                                    continue

        except httpx.TimeoutException:
            logger.error("Timeout while calling vLLM service")
            yield "Error: Request timeout"
        except httpx.HTTPStatusError as e:
            logger.error(f"HTTP error from vLLM service: {e.response.status_code}")
            yield f"Error: vLLM service error ({e.response.status_code})"
        except Exception as e:
            logger.error(f"Failed to generate response: {str(e)}")
            yield f"Error: {str(e)}"

    return StreamingResponse(
        stream_generator(),
        media_type="text/plain",
        headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
    )


async def generate_non_streaming_response(request: PromptRequest) -> PromptResponse:
    """Generate non-streaming response for backward compatibility"""
    try:
        # Prepare the request payload for vLLM OpenAI-compatible API
        vllm_payload = {
            "model": "placeholder",  # vLLM ignores this but requires it
            "messages": [{"role": "user", "content": request.prompt}],
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "top_p": request.top_p,
            "stream": False,
        }

        # Make request to vLLM service
        async with httpx.AsyncClient(timeout=60.0) as client:
            response = await client.post(
                f"{VLLM_URL}/v1/chat/completions", json=vllm_payload
            )
            response.raise_for_status()

            vllm_response = response.json()

            # Extract the generated text from OpenAI format
            choices = vllm_response.get("choices", [])
            if not choices:
                raise HTTPException(
                    status_code=500, detail="No choices in vLLM response"
                )

            message = choices[0].get("message", {})
            generated_text = message.get("content", "")

            if not generated_text:
                raise HTTPException(
                    status_code=500, detail="Empty response from vLLM service"
                )

            logger.info(
                f"Successfully generated response of length: {len(generated_text)}"
            )
            return PromptResponse(response=generated_text)

    except httpx.TimeoutException:
        logger.error("Timeout while calling vLLM service")
        raise HTTPException(status_code=504, detail="Request to vLLM service timed out")
    except httpx.HTTPStatusError as e:
        logger.error(f"HTTP error from vLLM service: {e.response.status_code}")
        raise HTTPException(
            status_code=502, detail=f"vLLM service error: {e.response.status_code}"
        )
    except Exception as e:
        logger.error(f"Failed to generate response: {str(e)}")
        raise HTTPException(
            status_code=500, detail=f"Failed to generate response: {str(e)}"
        )


if __name__ == "__main__":
    import uvicorn

    logger.info(f"Starting AI service on port {PORT}")
    logger.info(f"Using embedding model: {EMBEDDING_MODEL}")
    logger.info(f"Model path: {MODEL_PATH}")
    logger.info(f"Embedding dimensions: {EMBEDDING_DIMENSIONS}")
    logger.info(f"vLLM URL: {VLLM_URL}")

    uvicorn.run(app, host="0.0.0.0", port=PORT)
