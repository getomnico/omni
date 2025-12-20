import sys
import logging
from typing import Literal

from fastapi import FastAPI, HTTPException
from fastapi.responses import StreamingResponse
from pydantic import BaseModel
import asyncio
from concurrent.futures import ThreadPoolExecutor
import multiprocessing
from enum import IntEnum
from dataclasses import dataclass, field
import time

from providers import create_llm_provider
from embeddings import create_embedding_provider, DEFAULT_TASK
from tools import SearcherTool
from pdf_extractor import (
    PDFExtractionRequest,
    PDFExtractionResponse,
    extract_text_from_pdf,
)
from logger import setup_logging
from config import *  # Import all config variables
from db_config import (
    get_llm_config,
    get_embedding_config,
    VLLMConfig,
    AnthropicConfig,
    BedrockLLMConfig,
    LocalEmbeddingConfig,
    JinaEmbeddingConfig,
    OpenAIEmbeddingConfig,
    BedrockEmbeddingConfig,
)
from routers.chat import router as chat_router
from telemetry import init_telemetry
from embeddings.batch_processor import start_batch_processing
from storage import create_content_storage
from state import AppState

# Configure logging once at startup
setup_logging()

# Get logger for this module
logger = logging.getLogger(__name__)

app = FastAPI(title="Omni AI Service", version="0.1.0")

# Initialize typed application state
app.state = AppState()  # type: ignore[assignment]

# Initialize OpenTelemetry
init_telemetry(app, "omni-ai")

# Include routers
app.include_router(chat_router)

# Thread pool for async operations - scale based on CPU cores
# Reserve some cores for the web server and other processes
max_workers = max(2, min(multiprocessing.cpu_count() - 1, 8))
_executor = ThreadPoolExecutor(max_workers=max_workers)


# Priority queue for managing embedding requests
class Priority(IntEnum):
    HIGH = 1  # Searcher requests
    NORMAL = 2  # Default
    LOW = 3  # Indexer bulk requests


@dataclass(order=True)
class PrioritizedRequest:
    priority: int
    request_id: str = field(compare=False)
    request: "EmbeddingRequest" = field(compare=False)
    future: asyncio.Future = field(compare=False)
    timestamp: float = field(default_factory=time.time, compare=False)


# Global priority queue
_request_queue: asyncio.PriorityQueue = None
_queue_processor_task = None


class EmbeddingRequest(BaseModel):
    texts: list[str]
    task: str | None = DEFAULT_TASK
    chunk_size: int | None = 512  # Chunk size in tokens
    chunking_mode: str | None = "sentence"  # "sentence", "fixed", or "none"
    priority: Literal["high", "normal", "low"] | None = "normal"


class EmbeddingResponse(BaseModel):
    embeddings: list[list[list[float]]]
    chunks_count: list[int]  # Number of chunks per text
    chunks: list[list[tuple[int, int]]]  # Character offset spans for each chunk
    model_name: str


class PromptRequest(BaseModel):
    prompt: str
    max_tokens: int | None = 512
    temperature: float | None = 0.7
    top_p: float | None = 0.9
    stream: bool | None = True


class PromptResponse(BaseModel):
    response: str


@app.on_event("startup")
async def startup_event():
    """Initialize services on startup"""
    global _request_queue, _queue_processor_task

    # Initialize priority queue
    _request_queue = asyncio.PriorityQueue(maxsize=100)

    # Start queue processor task
    _queue_processor_task = asyncio.create_task(process_embedding_queue())

    logger.info(f"Initialized with {max_workers} thread pool workers")

    # Initialize embedding provider using database configuration (with env fallback)
    try:
        embedding_config = await get_embedding_config()
        logger.info(
            f"Loaded embedding configuration from database (provider: {embedding_config.provider})"
        )

        match embedding_config:
            case JinaEmbeddingConfig():
                if not embedding_config.jina_api_key:
                    raise ValueError(
                        "JINA_API_KEY is required when using Jina provider"
                    )

                logger.info(
                    f"Initializing JINA embedding provider with model: {embedding_config.jina_model}"
                )
                app.state.embedding_provider = create_embedding_provider(
                    "jina",
                    api_key=embedding_config.jina_api_key,
                    model=embedding_config.jina_model,
                    api_url=embedding_config.jina_api_url,
                    max_model_len=JINA_MAX_MODEL_LEN,
                )

            case BedrockEmbeddingConfig():
                logger.info(
                    f"Initializing Bedrock embedding provider with model: {embedding_config.bedrock_model_id}"
                )
                region_name = AWS_REGION if AWS_REGION else None
                app.state.embedding_provider = create_embedding_provider(
                    "bedrock",
                    model_id=embedding_config.bedrock_model_id,
                    region_name=region_name,
                    max_model_len=BEDROCK_EMBEDDING_MAX_MODEL_LEN,
                )

            case OpenAIEmbeddingConfig():
                api_key = embedding_config.openai_api_key or OPENAI_EMBEDDING_API_KEY
                if not api_key:
                    raise ValueError(
                        "OPENAI_EMBEDDING_API_KEY is required when using OpenAI provider"
                    )

                model = embedding_config.openai_model
                dimensions = (
                    embedding_config.openai_dimensions or OPENAI_EMBEDDING_DIMENSIONS
                )

                logger.info(
                    f"Initializing OpenAI embedding provider with model: {model}"
                )
                app.state.embedding_provider = create_embedding_provider(
                    "openai", api_key=api_key, model=model, dimensions=dimensions
                )

            case LocalEmbeddingConfig():
                base_url = embedding_config.local_base_url or LOCAL_EMBEDDINGS_URL
                model = embedding_config.local_model or LOCAL_EMBEDDINGS_MODEL

                logger.info(
                    f"Initializing local (vLLM) embedding provider with model: {model} at {base_url}"
                )
                app.state.embedding_provider = create_embedding_provider(
                    "local",
                    base_url=base_url,
                    model=model,
                    max_model_len=VLLM_EMBEDDINGS_MAX_MODEL_LEN,
                )

            case _:
                raise ValueError(
                    f"Unknown embedding provider: {embedding_config.provider}"
                )

        logger.info(
            f"Initialized {embedding_config.provider} embedding provider with model: {app.state.embedding_provider.get_model_name()}"
        )

        # Initialize LLM provider using database configuration (with env fallback)
        llm_config = await get_llm_config()
        logger.info(
            f"Loaded LLM configuration from database (provider: {llm_config.provider})"
        )

        match llm_config:
            case VLLMConfig():
                app.state.llm_provider = create_llm_provider(
                    "vllm", vllm_url=llm_config.vllm_url
                )
                logger.info(
                    f"Initialized vLLM provider with URL: {llm_config.vllm_url}"
                )

            case AnthropicConfig():
                app.state.llm_provider = create_llm_provider(
                    "anthropic",
                    api_key=llm_config.anthropic_api_key,
                    model=llm_config.primary_model_id,
                )
                logger.info(
                    f"Initialized Anthropic provider with model: {llm_config.primary_model_id}"
                )

            case BedrockLLMConfig():
                region_name = AWS_REGION if AWS_REGION else None
                app.state.llm_provider = create_llm_provider(
                    "bedrock",
                    model_id=llm_config.primary_model_id,
                    secondary_model_id=llm_config.secondary_model_id,
                    region_name=region_name,
                )
                logger.info(
                    f"Initialized AWS Bedrock provider with model: {llm_config.primary_model_id}"
                )
                if llm_config.secondary_model_id:
                    logger.info(
                        f"Using secondary model: {llm_config.secondary_model_id}"
                    )
                if region_name:
                    logger.info(f"Using AWS region: {region_name}")
                else:
                    logger.info("Using auto-detected AWS region from ECS environment")

            case _:
                raise ValueError(f"Unknown LLM provider: {llm_config.provider}")

        # Initialize searcher client
        app.state.searcher_tool = SearcherTool()
        logger.info("Initialized searcher client")

        # Initialize content storage and start batch processing
        # Batch processing is the main embeddings processor for document indexing
        app.state.content_storage = create_content_storage()
        logger.info("Initialized content storage for batch processing")

        # Start batch processing in background (always enabled)
        asyncio.create_task(
            start_batch_processing(
                app.state.content_storage,
                app.state.embedding_provider,
                embedding_config.provider,  # Pass provider type for routing
            )
        )
        logger.info(
            f"Started embedding batch processing with provider: {embedding_config.provider}"
        )

    except Exception as e:
        logger.error(f"Failed to initialize services: {e}")
        raise e


async def process_embedding_queue():
    """Process embedding requests from the priority queue"""
    while True:
        try:
            # Get the highest priority request
            prioritized_request = await _request_queue.get()
            request = prioritized_request.request
            future = prioritized_request.future

            # Log queue wait time for monitoring
            wait_time = time.time() - prioritized_request.timestamp
            if wait_time > 1.0:
                logger.warning(
                    f"Request {prioritized_request.request_id} waited {wait_time:.2f}s in queue (priority: {prioritized_request.priority})"
                )

            try:
                # Process the embedding request using the provider
                chunk_batch = await app.state.embedding_provider.generate_embeddings(
                    request.texts,
                    request.task,
                    request.chunk_size,
                    request.chunking_mode,
                )

                response = EmbeddingResponse(
                    embeddings=[
                        [c.embedding for c in chunks] for chunks in chunk_batch
                    ],
                    chunks_count=[len(chunks) for chunks in chunk_batch],
                    chunks=[[c.span for c in chunks] for chunks in chunk_batch],
                    model_name=app.state.embedding_provider.get_model_name(),
                )

                # Set the result on the future
                future.set_result(response)

            except Exception as e:
                logger.error(f"Failed to process embedding request: {e}")
                future.set_exception(e)

        except asyncio.CancelledError:
            logger.info("Queue processor task cancelled")
            break
        except Exception as e:
            logger.error(f"Error in queue processor: {e}")
            await asyncio.sleep(0.1)  # Brief pause on error


@app.on_event("shutdown")
async def shutdown_event():
    """Cleanup on shutdown"""
    global _queue_processor_task

    if _queue_processor_task:
        _queue_processor_task.cancel()
        try:
            await _queue_processor_task
        except asyncio.CancelledError:
            pass

    logger.info("AI service shutdown complete")


@app.get("/health")
async def health_check():
    """Health check endpoint"""
    # Check LLM provider health
    llm_health = False
    if hasattr(app.state, "llm_provider") and app.state.llm_provider:
        try:
            llm_health = await app.state.llm_provider.health_check()
        except Exception:
            llm_health = False

    # Get embedding model name from provider
    embedding_model = (
        app.state.embedding_provider.get_model_name()
        if hasattr(app.state, "embedding_provider")
        else "unknown"
    )

    # Get current configurations
    llm_config = await get_llm_config()
    embedding_config = await get_embedding_config()

    # Get LLM model ID based on config type
    match llm_config:
        case VLLMConfig():
            llm_model = llm_config.primary_model_id or "default"
        case AnthropicConfig() | BedrockLLMConfig():
            llm_model = llm_config.primary_model_id
        case _:
            llm_model = "unknown"

    return {
        "status": "healthy",
        "service": "ai",
        "embedding_provider": embedding_config.provider,
        "embedding_model": embedding_model,
        "port": PORT,
        "embedding_dimensions": EMBEDDING_DIMENSIONS,
        "llm_provider": llm_config.provider,
        "llm_model": llm_model,
        "llm_health": llm_health,
    }


@app.post("/embeddings", response_model=EmbeddingResponse)
async def generate_embeddings(request: EmbeddingRequest):
    """Generate embeddings for input texts using configurable chunking.

    Chunking behavior:
    - fixed mode: chunk_size sets the number of tokens per chunk
    - sentence mode: groups sentences until chunk_size tokens limit
    - none mode: embed entire text without chunking
    """
    logger.info(
        f"Generating embeddings for {len(request.texts)} texts with priority={request.priority}, chunking_mode={request.chunking_mode}, chunk_size={request.chunk_size}"
    )

    # Validate chunking method
    valid_chunking_modes = ["sentence", "fixed", "none"]
    if request.chunking_mode not in valid_chunking_modes:
        raise HTTPException(
            status_code=422,
            detail=f"Invalid chunking_mode: {request.chunking_mode}. Must be one of: {valid_chunking_modes}",
        )

    try:
        # Map priority string to enum
        priority_map = {
            "high": Priority.HIGH,
            "normal": Priority.NORMAL,
            "low": Priority.LOW,
        }
        priority = priority_map.get(request.priority, Priority.NORMAL)

        # Create a future for this request
        future = asyncio.Future()

        # Generate a unique request ID
        import uuid

        request_id = str(uuid.uuid4())[:8]

        # Create prioritized request
        prioritized_request = PrioritizedRequest(
            priority=priority, request_id=request_id, request=request, future=future
        )

        # Add to queue
        await _request_queue.put(prioritized_request)

        # Log queue size if it's getting large
        queue_size = _request_queue.qsize()
        if queue_size > 10:
            logger.warning(f"Embedding queue size: {queue_size}")

        # Wait for the result
        response = await future

        logger.info(
            f"Generated these many chunks for each input text: {response.chunks_count}"
        )
        return response

    except Exception as e:
        logger.error(f"Failed to generate embeddings: {str(e)}")
        raise HTTPException(status_code=500, detail=str(e))


@app.post("/prompt")
async def generate_response(request: PromptRequest):
    """Generate a response from the configured LLM provider with streaming support"""
    if not hasattr(app.state, "llm_provider") or not app.state.llm_provider:
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
            async for event in app.state.llm_provider.stream_response(
                request.prompt,
                max_tokens=request.max_tokens,
                temperature=request.temperature,
                top_p=request.top_p,
            ):
                # Extract text content from MessageStreamEvent
                if event.type == "content_block_delta":
                    if event.delta.text:
                        yield event.delta.text
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
        _executor, extract_text_from_pdf, request.pdf_bytes
    )

    if result.error:
        logger.warning(f"PDF extraction completed with error: {result.error}")
    else:
        logger.info(
            f"PDF extraction successful: {result.page_count} pages, {len(result.text)} characters"
        )

    return result


async def generate_non_streaming_response(request: PromptRequest) -> PromptResponse:
    """Generate non-streaming response for backward compatibility"""
    if not hasattr(app.state, "llm_provider") or not app.state.llm_provider:
        raise HTTPException(status_code=500, detail="LLM provider not initialized")

    try:
        generated_text = await app.state.llm_provider.generate_response(
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
    logger.info(f"Embedding Provider: {EMBEDDING_PROVIDER}")
    logger.info(f"Embedding dimensions: {EMBEDDING_DIMENSIONS}")
    logger.info(f"LLM Provider: {LLM_PROVIDER}")
    if LLM_PROVIDER == "vllm":
        logger.info(f"vLLM URL: {VLLM_URL}")
    elif LLM_PROVIDER == "anthropic":
        logger.info(f"Anthropic Model: {ANTHROPIC_MODEL}")
        logger.info(f"Anthropic Max Tokens: {ANTHROPIC_MAX_TOKENS}")
    elif LLM_PROVIDER == "bedrock":
        logger.info(f"Bedrock Model ID: {BEDROCK_MODEL_ID}")
        logger.info(f"AWS Region: {AWS_REGION if AWS_REGION else 'Auto-detected'}")

    uvicorn.run(app, host="0.0.0.0", port=PORT)
