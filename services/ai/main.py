import os
import sys
from fastapi import FastAPI, HTTPException
from pydantic import BaseModel
from typing import List, Optional
import logging
import asyncio
import httpx
from concurrent.futures import ThreadPoolExecutor

from embeddings import (
    load_model,
    generate_embeddings_sync,
    TASK,
)

def get_required_env(key: str) -> str:
    """Get required environment variable with validation"""
    value = os.getenv(key)
    if not value:
        print(f"ERROR: Required environment variable '{key}' is not set", file=sys.stderr)
        print("Please set this variable in your .env file or environment", file=sys.stderr)
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
EMBEDDING_DIMENSIONS = validate_embedding_dimensions(get_required_env("EMBEDDING_DIMENSIONS"))
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
    task: Optional[str] = TASK  # Allow different tasks
    chunk_size: Optional[int] = 512  # Chunk size for fixed-size chunking
    chunking_mode: Optional[str] = "sentence"  # "sentence" or "fixed"


class EmbeddingResponse(BaseModel):
    embeddings: List[List[List[float]]]
    chunks_count: List[int]  # Number of chunks per text


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
        "embedding_dimensions": EMBEDDING_DIMENSIONS
    }


@app.post("/embeddings", response_model=EmbeddingResponse)
async def generate_embeddings(request: EmbeddingRequest):
    """Generate embeddings for input texts using configurable chunking"""
    logger.info(
        f"Generating embeddings for {len(request.texts)} texts with chunking_mode={request.chunking_mode}, chunk_size={request.chunk_size}"
    )

    # Validate chunking method
    valid_chunking_modes = ["sentence", "fixed"]
    if request.chunking_mode not in valid_chunking_modes:
        raise HTTPException(
            status_code=422,
            detail=f"Invalid chunking_mode: {request.chunking_mode}. Must be one of: {valid_chunking_modes}"
        )

    try:
        # Run embedding generation in thread pool to avoid blocking
        embeddings, chunks_count = await asyncio.get_event_loop().run_in_executor(
            _executor,
            generate_embeddings_sync,
            request.texts,
            request.task,
            request.chunk_size,
            request.chunking_mode,
        )

        logger.info(f"Generated embeddings with chunks: {chunks_count}")
        return EmbeddingResponse(embeddings=embeddings, chunks_count=chunks_count)

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


@app.post("/prompt", response_model=PromptResponse)
async def generate_response(request: PromptRequest):
    """Generate a response from the vLLM model for any given prompt"""
    logger.info(f"Generating response for prompt: {request.prompt[:50]}...")
    
    try:
        # Prepare the request payload for vLLM
        vllm_payload = {
            "prompt": request.prompt,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "top_p": request.top_p,
        }
        
        # Make request to vLLM service
        async with httpx.AsyncClient(timeout=30.0) as client:
            response = await client.post(
                f"{VLLM_URL}/generate",
                json=vllm_payload
            )
            response.raise_for_status()
            
            vllm_response = response.json()
            
            # Extract the generated text from vLLM response
            # vLLM typically returns {"text": [generated_text]}
            generated_text = vllm_response.get("text", [""])[0] if "text" in vllm_response else ""
            
            if not generated_text:
                raise HTTPException(status_code=500, detail="Empty response from vLLM service")
            
            logger.info(f"Successfully generated response of length: {len(generated_text)}")
            return PromptResponse(response=generated_text)
            
    except httpx.TimeoutException:
        logger.error("Timeout while calling vLLM service")
        raise HTTPException(status_code=504, detail="Request to vLLM service timed out")
    except httpx.HTTPStatusError as e:
        logger.error(f"HTTP error from vLLM service: {e.response.status_code} - {e.response.text}")
        raise HTTPException(
            status_code=502, 
            detail=f"vLLM service error: {e.response.status_code}"
        )
    except Exception as e:
        logger.error(f"Failed to generate response: {str(e)}")
        raise HTTPException(status_code=500, detail=f"Failed to generate response: {str(e)}")


if __name__ == "__main__":
    import uvicorn
    
    logger.info(f"Starting AI service on port {PORT}")
    logger.info(f"Using embedding model: {EMBEDDING_MODEL}")
    logger.info(f"Model path: {MODEL_PATH}")
    logger.info(f"Embedding dimensions: {EMBEDDING_DIMENSIONS}")
    logger.info(f"vLLM URL: {VLLM_URL}")

    uvicorn.run(app, host="0.0.0.0", port=PORT)
