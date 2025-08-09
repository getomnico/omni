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
import base64

from embeddings_v2 import (
    load_model,
    generate_embeddings_sync,
    DEFAULT_TASK,
)
from providers import create_llm_provider, LLMProvider
from pdf_extractor import PDFExtractionRequest, PDFExtractionResponse, extract_text_from_pdf


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
REDIS_URL = get_required_env("REDIS_URL")
DATABASE_URL = get_required_env("DATABASE_URL")

# LLM Provider configuration
LLM_PROVIDER = get_optional_env("LLM_PROVIDER", "vllm").lower()
VLLM_URL = get_optional_env("VLLM_URL", "http://vllm:8000")  # Make optional
ANTHROPIC_API_KEY = get_optional_env("ANTHROPIC_API_KEY", "")
ANTHROPIC_MODEL = get_optional_env("ANTHROPIC_MODEL", "claude-sonnet-4-20250514")
ANTHROPIC_MAX_TOKENS = int(get_optional_env("ANTHROPIC_MAX_TOKENS", "4096"))

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

app = FastAPI(title="Clio AI Service", version="0.1.0")

# Thread pool for async operations
_executor = ThreadPoolExecutor(max_workers=2)

# Global LLM provider instance
llm_provider: Optional[LLMProvider] = None


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
    """Load model and initialize LLM provider on startup"""
    global llm_provider

    # Load embedding model
    await asyncio.get_event_loop().run_in_executor(_executor, load_model)

    # Initialize LLM provider
    try:
        if LLM_PROVIDER == "vllm":
            if not VLLM_URL:
                raise ValueError("VLLM_URL is required when using vLLM provider")
            llm_provider = create_llm_provider("vllm", vllm_url=VLLM_URL)
            logger.info(f"Initialized vLLM provider with URL: {VLLM_URL}")
        elif LLM_PROVIDER == "anthropic":
            if not ANTHROPIC_API_KEY:
                raise ValueError(
                    "ANTHROPIC_API_KEY is required when using Anthropic provider"
                )
            llm_provider = create_llm_provider(
                "anthropic", api_key=ANTHROPIC_API_KEY, model=ANTHROPIC_MODEL
            )
            logger.info(f"Initialized Anthropic provider with model: {ANTHROPIC_MODEL}")
        else:
            raise ValueError(f"Unknown LLM provider: {LLM_PROVIDER}")
    except Exception as e:
        logger.error(f"Failed to initialize LLM provider: {e}")
        raise e


@app.get("/health")
async def health_check():
    """Health check endpoint"""
    global llm_provider

    # Check LLM provider health
    llm_health = False
    if llm_provider:
        try:
            llm_health = await llm_provider.health_check()
        except Exception:
            llm_health = False

    return {
        "status": "healthy",
        "service": "ai",
        "model": EMBEDDING_MODEL,
        "port": PORT,
        "embedding_dimensions": EMBEDDING_DIMENSIONS,
        "llm_provider": LLM_PROVIDER,
        "llm_health": llm_health,
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
    """Generate a response from the configured LLM provider with streaming support"""
    global llm_provider

    if not llm_provider:
        raise HTTPException(status_code=500, detail="LLM provider not initialized")

    logger.info(
        f"Generating response for prompt: {request.prompt[:50]}... (stream={request.stream}, provider={LLM_PROVIDER})"
    )

    if not request.stream:
        # Non-streaming response (keep for backward compatibility)
        return await generate_non_streaming_response(request)

    # Streaming response
    async def stream_generator():
        try:
            async for chunk in llm_provider.stream_response(
                request.prompt,
                max_tokens=request.max_tokens,
                temperature=request.temperature,
                top_p=request.top_p,
            ):
                yield chunk
        except Exception as e:
            logger.error(f"Failed to generate streaming response: {str(e)}")
            yield f"Error: {str(e)}"

    return StreamingResponse(
        stream_generator(),
        media_type="text/plain",
        headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
    )


@app.post("/extract_pdf", response_model=PDFExtractionResponse)
async def extract_pdf(request: PDFExtractionRequest):
    """Extract text from a PDF file"""
    logger.info(f"Extracting text from PDF ({len(request.pdf_bytes)} bytes)")
    
    # Run PDF extraction in executor to avoid blocking
    loop = asyncio.get_event_loop()
    result = await loop.run_in_executor(
        _executor,
        extract_text_from_pdf,
        request.pdf_bytes
    )
    
    if result.error:
        logger.warning(f"PDF extraction completed with error: {result.error}")
    else:
        logger.info(f"PDF extraction successful: {result.page_count} pages, {len(result.text)} characters")
    
    return result


async def generate_non_streaming_response(request: PromptRequest) -> PromptResponse:
    """Generate non-streaming response for backward compatibility"""
    global llm_provider

    if not llm_provider:
        raise HTTPException(status_code=500, detail="LLM provider not initialized")

    try:
        generated_text = await llm_provider.generate_response(
            request.prompt,
            max_tokens=request.max_tokens,
            temperature=request.temperature,
            top_p=request.top_p,
        )

        logger.info(f"Successfully generated response of length: {len(generated_text)}")
        return PromptResponse(response=generated_text)

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
    logger.info(f"LLM Provider: {LLM_PROVIDER}")
    if LLM_PROVIDER == "vllm":
        logger.info(f"vLLM URL: {VLLM_URL}")
    elif LLM_PROVIDER == "anthropic":
        logger.info(f"Anthropic Model: {ANTHROPIC_MODEL}")
        logger.info(f"Anthropic Max Tokens: {ANTHROPIC_MAX_TOKENS}")

    uvicorn.run(app, host="0.0.0.0", port=PORT)
