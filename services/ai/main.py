import sys
import logging
from fastapi import FastAPI, HTTPException
from fastapi.responses import StreamingResponse
from pydantic import BaseModel
from typing import List, Optional, Tuple, Literal
import asyncio
from concurrent.futures import ThreadPoolExecutor
import multiprocessing
from enum import IntEnum
from dataclasses import dataclass, field
import time

from providers import create_llm_provider
from embeddings import create_embedding_provider, DEFAULT_TASK
from tools import SearcherTool
from pdf_extractor import PDFExtractionRequest, PDFExtractionResponse, extract_text_from_pdf
from logger import setup_logging
from config import *  # Import all config variables
from routers.chat import router as chat_router
from telemetry import init_telemetry
from embeddings.batch_processor import start_batch_processing
from storage import create_content_storage

# Configure logging once at startup
setup_logging()

# Get logger for this module
logger = logging.getLogger(__name__)

app = FastAPI(title="Omni AI Service", version="0.1.0")

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
    HIGH = 1    # Searcher requests
    NORMAL = 2  # Default
    LOW = 3     # Indexer bulk requests

@dataclass(order=True)
class PrioritizedRequest:
    priority: int
    request_id: str = field(compare=False)
    request: 'EmbeddingRequest' = field(compare=False)
    future: asyncio.Future = field(compare=False)
    timestamp: float = field(default_factory=time.time, compare=False)

# Global priority queue
_request_queue: asyncio.PriorityQueue = None
_queue_processor_task = None

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
    priority: Optional[Literal["high", "normal", "low"]] = "normal"  # Request priority


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
    """Initialize services on startup"""
    global _request_queue, _queue_processor_task

    # Initialize priority queue
    _request_queue = asyncio.PriorityQueue(maxsize=100)

    # Start queue processor task
    _queue_processor_task = asyncio.create_task(process_embedding_queue())

    logger.info(f"Initialized with {max_workers} thread pool workers")

    # Initialize embedding provider
    try:
        if EMBEDDING_PROVIDER == "jina":
            logger.info(f"Initializing JINA embedding provider with model: {JINA_MODEL}")
            app.state.embedding_provider = create_embedding_provider(
                EMBEDDING_PROVIDER,
                api_key=JINA_API_KEY,
                model=JINA_MODEL,
                api_url=JINA_API_URL
            )
        elif EMBEDDING_PROVIDER == "bedrock":
            logger.info(f"Initializing Bedrock embedding provider with model: {BEDROCK_EMBEDDING_MODEL_ID}")
            region_name = AWS_REGION if AWS_REGION else None
            app.state.embedding_provider = create_embedding_provider(
                EMBEDDING_PROVIDER,
                model_id=BEDROCK_EMBEDDING_MODEL_ID,
                region_name=region_name
            )
        else:
            raise ValueError(f"Unknown embedding provider: {EMBEDDING_PROVIDER}")

        logger.info(f"Initialized {EMBEDDING_PROVIDER} embedding provider with model: {app.state.embedding_provider.get_model_name()}")

        # Initialize LLM provider
        if LLM_PROVIDER == "vllm":
            if not VLLM_URL:
                raise ValueError("VLLM_URL is required when using vLLM provider")
            app.state.llm_provider = create_llm_provider("vllm", vllm_url=VLLM_URL)
            logger.info(f"Initialized vLLM provider with URL: {VLLM_URL}")
        elif LLM_PROVIDER == "anthropic":
            if not ANTHROPIC_API_KEY:
                raise ValueError(
                    "ANTHROPIC_API_KEY is required when using Anthropic provider"
                )
            app.state.llm_provider = create_llm_provider(
                "anthropic", api_key=ANTHROPIC_API_KEY, model=ANTHROPIC_MODEL
            )
            logger.info(f"Initialized Anthropic provider with model: {ANTHROPIC_MODEL}")
        elif LLM_PROVIDER == "bedrock":
            region_name = AWS_REGION if AWS_REGION else None
            app.state.llm_provider = create_llm_provider(
                "bedrock", model_id=BEDROCK_MODEL_ID, secondary_model_id=TITLE_GENERATION_MODEL_ID, region_name=region_name
            )
            logger.info(f"Initialized AWS Bedrock provider with model: {BEDROCK_MODEL_ID}")
            if region_name:
                logger.info(f"Using AWS region: {region_name}")
            else:
                logger.info("Using auto-detected AWS region from ECS environment")
        else:
            raise ValueError(f"Unknown LLM provider: {LLM_PROVIDER}")

        # Initialize searcher client
        app.state.searcher_tool = SearcherTool()
        logger.info("Initialized searcher client")

        # Initialize content storage and start batch processing if enabled
        if ENABLE_EMBEDDING_BATCH_INFERENCE:
            app.state.content_storage = create_content_storage()
            logger.info("Initialized content storage for batch processing")

            # Start batch processing in background
            asyncio.create_task(start_batch_processing(
                app.state.content_storage,
                app.state.embedding_provider
            ))
            logger.info("Started embedding batch processing")

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
                    request.n_sentences,
                )

                response = EmbeddingResponse(
                    embeddings=[[c.embedding for c in chunks] for chunks in chunk_batch],
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
    if hasattr(app.state, 'llm_provider') and app.state.llm_provider:
        try:
            llm_health = await app.state.llm_provider.health_check()
        except Exception:
            llm_health = False

    # Get embedding model name from provider
    embedding_model = app.state.embedding_provider.get_model_name() if hasattr(app.state, 'embedding_provider') else "unknown"

    return {
        "status": "healthy",
        "service": "ai",
        "embedding_provider": EMBEDDING_PROVIDER,
        "embedding_model": embedding_model,
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
        f"Generating embeddings for {len(request.texts)} texts with priority={request.priority}, chunking_mode={request.chunking_mode}, chunk_size={request.chunk_size}, n_sentences={request.n_sentences}"
    )

    # Validate chunking method
    valid_chunking_modes = ["sentence", "fixed", "semantic", "none"]
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
            "low": Priority.LOW
        }
        priority = priority_map.get(request.priority, Priority.NORMAL)
        
        # Create a future for this request
        future = asyncio.Future()
        
        # Generate a unique request ID
        import uuid
        request_id = str(uuid.uuid4())[:8]
        
        # Create prioritized request
        prioritized_request = PrioritizedRequest(
            priority=priority,
            request_id=request_id,
            request=request,
            future=future
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
    if not hasattr(app.state, 'llm_provider') or not app.state.llm_provider:
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
                if event.type == 'content_block_delta':
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
    if not hasattr(app.state, 'llm_provider') or not app.state.llm_provider:
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
