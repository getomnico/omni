import logging
import os
import torch
import torch.nn.functional as F
from transformers import AutoModel, AutoTokenizer
import numpy as np
from typing import List, Tuple, Dict
import threading
from concurrent.futures import ThreadPoolExecutor
import code
from chunking import Chunker

logger = logging.getLogger(__name__)

# Global variables for model and tokenizer
_model = None
_tokenizer = None
_model_lock = threading.Lock()

# Global variables for chunkers
_chunkers: Dict[str, Chunker] = {}
_chunker_lock = threading.Lock()

# Model configuration
TASK = "retrieval.passage"
MODEL_NAME = os.getenv("EMBEDDING_MODEL", "jinaai/jina-embeddings-v3")
CHUNKING_MODEL_NAME = "BAAI/bge-small-en"
MAX_LENGTH = 8192  # Jina v3 supports up to 8K tokens


def load_model():
    """Load the Jina embeddings model and tokenizer"""
    global _model, _tokenizer

    with _model_lock:
        if _model is None:
            logger.info(f"Loading model {MODEL_NAME}...")
            _model = AutoModel.from_pretrained(MODEL_NAME, trust_remote_code=True)
            _tokenizer = AutoTokenizer.from_pretrained(
                MODEL_NAME, trust_remote_code=True
            )

            # Move to GPU if available
            if torch.cuda.is_available():
                _model = _model.cuda()
                logger.info("Model loaded on GPU")
            else:
                logger.info("Model loaded on CPU")

    return _model, _tokenizer


def get_chunker(chunking_mode: str) -> Chunker:
    """Get or create a chunker instance for the given mode"""
    global _chunkers

    with _chunker_lock:
        if chunking_mode not in _chunkers:
            logger.info(f"Creating new chunker for mode: {chunking_mode}")
            _chunkers[chunking_mode] = Chunker(chunking_mode)
        else:
            logger.debug(f"Reusing existing chunker for mode: {chunking_mode}")

    return _chunkers[chunking_mode]


def apply_chunking_to_embeddings(
    token_embeddings: torch.Tensor, chunk_spans: List[Tuple[int, int]]
) -> List[torch.Tensor]:
    """Apply chunking to token embeddings using provided chunk spans"""
    chunks = []

    for start_idx, end_idx in chunk_spans:
        # Extract embeddings for this chunk span
        # Ensure indices are within bounds
        start_idx = max(0, start_idx)
        end_idx = min(token_embeddings.shape[1], end_idx)

        if start_idx < end_idx:  # Valid span
            chunk_embeddings = token_embeddings[:, start_idx:end_idx, :]

            # Mean pooling for chunk representation
            chunk_embedding = torch.mean(chunk_embeddings, dim=1)
            # Normalize to unit norm
            chunk_embedding = F.normalize(chunk_embedding, p=2, dim=-1)
            chunks.append(chunk_embedding)

    return chunks


def generate_embeddings_sync(
    texts: List[str], task: str, chunk_size: int, chunking_mode: str
) -> Tuple[List[List[float]], List[int], List[List[Tuple[int, int]]]]:
    """Synchronous embedding generation with configurable chunking"""
    try:
        logger.info(f"Starting embedding generation for {len(texts)} texts")
        model, tokenizer = load_model()
        logger.info("Model and tokenizer loaded successfully")

        # Get cached chunker instance
        chunker = get_chunker(chunking_mode)

        # Batch tokenization
        logger.info("Tokenizing all texts in batch...")
        inputs = tokenizer(
            texts,
            return_tensors="pt",
            truncation=True,
            max_length=MAX_LENGTH,
            padding=True,
        )
        logger.info(
            f"Batch tokenization complete, input_ids shape: {inputs['input_ids'].shape}"
        )

        if torch.cuda.is_available():
            logger.info("Moving batch inputs to GPU...")
            inputs = {k: v.cuda() for k, v in inputs.items()}
            logger.info("Batch inputs moved to GPU")

        # Get task ID for adapter
        logger.info(f"Getting task ID for task: {task}")
        task_id = model._adaptation_map.get(task, model._adaptation_map[TASK])
        num_examples = inputs["input_ids"].shape[0]
        logger.info(f"Task ID: {task_id}, num_examples: {num_examples}")

        device = model.device
        logger.info(f"Model device: {device}")
        adapter_mask = torch.full(
            (num_examples,), task_id, dtype=torch.int32, device=device
        )
        logger.info(
            f"Created adapter_mask: shape={adapter_mask.shape}, dtype={adapter_mask.dtype}, device={adapter_mask.device}"
        )

        # Single forward pass for all texts
        logger.info("Starting batch model forward pass...")
        with torch.no_grad():
            model_output = model(**inputs, adapter_mask=adapter_mask, return_dict=True)
            logger.info("Batch model forward pass completed")
            batch_token_embeddings = model_output.last_hidden_state
            logger.info(
                f"Batch token embeddings shape: {batch_token_embeddings.shape}, dtype: {batch_token_embeddings.dtype}"
            )

        # Process each text's embeddings from the batch
        all_embeddings = []
        chunks_count = []
        all_chunk_spans = []

        for i, text in enumerate(texts):
            logger.info(f"Processing embeddings for text {i+1}/{len(texts)}")

            # Extract embeddings for this specific text from the batch
            token_embeddings = batch_token_embeddings[i : i + 1]  # Keep batch dimension

            # Apply chunking based on the selected mode
            logger.info(f"Applying chunking mode: {chunking_mode}")

            # Get chunk spans using the Chunker
            if chunking_mode == "sentence":
                # Use sentence-based chunking with 1 sentence per chunk
                logger.info("Processing sentence-based chunking...")
                token_spans, char_spans = chunker.chunk(
                    text,
                    tokenizer,
                    n_sentences=1,
                    embedding_model_name=CHUNKING_MODEL_NAME,
                )
                logger.info(f"Found {len(token_spans)} sentence spans")
            elif chunking_mode == "semantic":
                # Use semantic chunking
                logger.info("Processing semantic chunking...")
                token_spans, char_spans = chunker.chunk(
                    text, tokenizer, embedding_model_name=CHUNKING_MODEL_NAME
                )
                logger.info(f"Found {len(token_spans)} semantic spans")
            else:
                # Use fixed-size chunking (default)
                logger.info("Processing fixed-size chunking...")
                token_spans, char_spans = chunker.chunk(
                    text,
                    tokenizer,
                    chunk_size=chunk_size,
                    embedding_model_name=CHUNKING_MODEL_NAME,
                )
                logger.info(f"Found {len(token_spans)} fixed-size spans")

            # Apply the chunking to embeddings using token spans
            chunk_embeddings = apply_chunking_to_embeddings(
                token_embeddings, token_spans
            )
            logger.info(f"Chunking produced {len(chunk_embeddings)} chunks")

            # Convert to numpy and store
            logger.info("Converting embeddings to numpy...")
            text_embeddings = []
            for j, chunk_emb in enumerate(chunk_embeddings):
                logger.info(f"Converting chunk {j+1}/{len(chunk_embeddings)} to numpy")
                # Convert BFloat16 to Float32 before numpy conversion
                chunk_emb_np = chunk_emb.float().cpu().numpy().tolist()
                text_embeddings.append(chunk_emb_np[0])

            logger.info(
                f"Adding all_embeddings with text_embeddings of len {len(text_embeddings)}"
            )
            logger.info(
                f"Text embeddings array content lens: {[len(x) for x in text_embeddings]}"
            )
            all_embeddings.append(text_embeddings)
            chunks_count.append(len(chunk_embeddings))
            all_chunk_spans.append(char_spans)
            logger.info(f"Completed processing text {i+1}/{len(texts)}")

        logger.info("All embeddings generated successfully")
        return all_embeddings, chunks_count, all_chunk_spans

    except Exception as e:
        logger.error(f"Error generating embeddings: {str(e)}")
        import traceback

        logger.error(f"Full traceback: {traceback.format_exc()}")
        raise Exception(f"Embedding generation failed: {str(e)}")
