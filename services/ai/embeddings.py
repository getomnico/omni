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

    # Skip chunker creation for "none" mode
    if chunking_mode == "none":
        return None

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


def process_single_text(
    text: str, model, tokenizer, chunker, task: str, chunk_size: int, chunking_mode: str
) -> Tuple[List[List[float]], int, List[Tuple[int, int]]]:
    """Process a single text, handling long documents by segmenting if needed"""

    # Handle empty text early
    if not text or not text.strip():
        logger.warning("Empty text provided, returning empty results")
        return [], 0, []

    # First, check if text needs segmentation due to length
    test_tokens = tokenizer(text, return_tensors="pt", truncation=False, padding=False)
    total_tokens = test_tokens["input_ids"].shape[1]

    if total_tokens <= MAX_LENGTH:
        # Process normally
        logger.info(f"Text has {total_tokens} tokens, processing as single segment")
        return process_text_segment(
            text, model, tokenizer, chunker, task, chunk_size, chunking_mode, 0
        )
    else:
        # Text is too long, need to segment it
        logger.info(
            f"Text has {total_tokens} tokens, exceeding max {MAX_LENGTH}. Segmenting..."
        )

        # Split text into segments that fit within MAX_LENGTH
        segments = split_text_into_segments(text, tokenizer, MAX_LENGTH)
        logger.info(f"Split text into {len(segments)} segments")

        all_embeddings = []
        all_chunk_spans = []

        # Since we have overlapping segments, we need to track the actual position
        # in the original text more carefully
        processed_chars = 0

        for seg_idx, segment in enumerate(segments):
            seg_tokens = tokenizer(
                segment, return_tensors="pt", truncation=False, padding=False
            )
            seg_token_count = seg_tokens["input_ids"].shape[1]
            logger.info(
                f"Processing segment {seg_idx + 1}/{len(segments)} with {seg_token_count} tokens"
            )

            # For overlapping segments, we need to find the actual position in original text
            if seg_idx == 0:
                segment_start = 0
            else:
                # Find where this segment's new content starts in the original text
                # This is tricky with overlapping segments, so we use a simpler approach:
                # Process each segment independently and adjust spans later
                segment_start = processed_chars

            seg_embeddings, seg_chunks_count, seg_chunk_spans = process_text_segment(
                segment,
                model,
                tokenizer,
                chunker,
                task,
                chunk_size,
                chunking_mode,
                segment_start,
            )

            logger.info(f"Segment {seg_idx + 1} produced {seg_chunks_count} chunks")

            all_embeddings.extend(seg_embeddings)
            all_chunk_spans.extend(seg_chunk_spans)

            # Update processed chars - this is an approximation
            # In practice, chunk spans from overlapping segments may not align perfectly
            # with the original text, but they'll be close enough for most use cases
            if seg_idx == 0:
                processed_chars = len(segment)
            else:
                # Add only the non-overlapping part
                processed_chars += len(segment) // 2

        logger.info(f"Total chunks generated from all segments: {len(all_embeddings)}")
        return all_embeddings, len(all_embeddings), all_chunk_spans


def split_text_into_segments(text: str, tokenizer, max_length: int) -> List[str]:
    """Split text into segments that fit within max_length tokens"""
    import re

    # Split text into sentences for better boundary detection
    sentence_pattern = r"(?<=[.!?])\s+|\n\n+"
    sentences = re.split(sentence_pattern, text)

    segments = []
    current_segment = []
    current_tokens = 0

    # Reserve tokens for special tokens and padding
    effective_max_length = max_length - 50

    for sentence in sentences:
        # Skip empty sentences
        if not sentence.strip():
            continue

        # Tokenize the sentence to get accurate token count
        sentence_tokens = tokenizer.encode(sentence, add_special_tokens=False)
        sentence_token_count = len(sentence_tokens)

        # If a single sentence exceeds max length, we need to split it
        if sentence_token_count > effective_max_length:
            # First, add any accumulated sentences as a segment
            if current_segment:
                segments.append(" ".join(current_segment))
                current_segment = []
                current_tokens = 0

            # Split the long sentence into smaller parts
            words = sentence.split()
            temp_segment = []
            temp_tokens = 0

            for word in words:
                word_tokens = len(tokenizer.encode(word, add_special_tokens=False))
                if temp_tokens + word_tokens > effective_max_length:
                    if temp_segment:
                        segments.append(" ".join(temp_segment))
                    temp_segment = [word]
                    temp_tokens = word_tokens
                else:
                    temp_segment.append(word)
                    temp_tokens += word_tokens

            if temp_segment:
                segments.append(" ".join(temp_segment))

        # Check if adding this sentence would exceed the limit
        elif current_tokens + sentence_token_count > effective_max_length:
            # Save current segment and start a new one
            if current_segment:
                segments.append(" ".join(current_segment))
            current_segment = [sentence]
            current_tokens = sentence_token_count
        else:
            # Add sentence to current segment
            current_segment.append(sentence)
            current_tokens += sentence_token_count

    # Don't forget the last segment
    if current_segment:
        segments.append(" ".join(current_segment))

    # Add overlap between segments for better context preservation
    overlapped_segments = []
    overlap_sentences = 2  # Number of sentences to overlap

    for i, segment in enumerate(segments):
        if i == 0:
            overlapped_segments.append(segment)
        else:
            # Get last few sentences from previous segment for context
            prev_sentences = segments[i - 1].split(". ")
            if len(prev_sentences) > overlap_sentences:
                # Take only the last few sentences as overlap
                overlap_text = ". ".join(prev_sentences[-overlap_sentences:]) + ". "
                # Check that overlap doesn't make segment too long
                overlap_tokens = len(
                    tokenizer.encode(overlap_text, add_special_tokens=False)
                )
                segment_tokens = len(
                    tokenizer.encode(segment, add_special_tokens=False)
                )

                if overlap_tokens + segment_tokens < effective_max_length:
                    overlapped_segments.append(overlap_text + segment)
                else:
                    # Overlap would make segment too long, skip it
                    overlapped_segments.append(segment)
            else:
                # Previous segment is short, include it all if it fits
                combined = segments[i - 1] + " " + segment
                combined_tokens = len(
                    tokenizer.encode(combined, add_special_tokens=False)
                )
                if combined_tokens < effective_max_length:
                    overlapped_segments.append(combined)
                else:
                    overlapped_segments.append(segment)

    return overlapped_segments


def process_text_segment(
    text: str,
    model,
    tokenizer,
    chunker,
    task: str,
    chunk_size: int,
    chunking_mode: str,
    char_offset: int = 0,
) -> Tuple[List[List[float]], int, List[Tuple[int, int]]]:
    """Process a single text segment and return embeddings"""

    # Tokenize the text
    inputs = tokenizer(
        text,
        return_tensors="pt",
        truncation=True,
        max_length=MAX_LENGTH,
        padding=True,
    )

    if torch.cuda.is_available():
        inputs = {k: v.cuda() for k, v in inputs.items()}

    # Get task ID for adapter
    task_id = model._adaptation_map.get(task, model._adaptation_map[TASK])
    device = model.device
    adapter_mask = torch.full((1,), task_id, dtype=torch.int32, device=device)

    # Forward pass
    with torch.no_grad():
        model_output = model(**inputs, adapter_mask=adapter_mask, return_dict=True)
        token_embeddings = model_output.last_hidden_state

    # Apply chunking
    if chunking_mode == "none":
        # No chunking - use the entire text as a single chunk
        attention_mask = inputs["attention_mask"][0]
        token_length = attention_mask.sum().item()
        token_spans = [(0, token_length)]
        char_spans = [(char_offset, char_offset + len(text))]
    else:
        # Use the chunker
        if chunking_mode == "sentence":
            token_spans, local_char_spans = chunker.chunk(
                text, tokenizer, n_sentences=1, embedding_model_name=CHUNKING_MODEL_NAME
            )
        elif chunking_mode == "semantic":
            token_spans, local_char_spans = chunker.chunk(
                text, tokenizer, embedding_model_name=CHUNKING_MODEL_NAME
            )
        else:  # fixed
            token_spans, local_char_spans = chunker.chunk(
                text,
                tokenizer,
                chunk_size=chunk_size,
                embedding_model_name=CHUNKING_MODEL_NAME,
            )

        # Adjust character spans with offset
        char_spans = [
            (start + char_offset, end + char_offset) for start, end in local_char_spans
        ]

    # Apply chunking to embeddings
    chunk_embeddings = apply_chunking_to_embeddings(token_embeddings, token_spans)

    # Convert to numpy
    text_embeddings = []
    valid_char_spans = []

    for i, (chunk_emb, char_span) in enumerate(zip(chunk_embeddings, char_spans)):
        # Validate char span
        if char_span[0] < char_span[1]:
            chunk_emb_np = chunk_emb.float().cpu().numpy().tolist()
            text_embeddings.append(chunk_emb_np[0])
            valid_char_spans.append(char_span)
        else:
            logger.warning(f"Skipping invalid chunk span: {char_span}")

    return text_embeddings, len(text_embeddings), valid_char_spans


def generate_embeddings_sync(
    texts: List[str], task: str, chunk_size: int, chunking_mode: str
) -> Tuple[List[List[float]], List[int], List[List[Tuple[int, int]]]]:
    """Synchronous embedding generation with configurable chunking"""
    try:
        logger.info(f"Starting embedding generation for {len(texts)} texts")
        model, tokenizer = load_model()
        logger.info("Model and tokenizer loaded successfully")

        # Get cached chunker instance (None for "none" mode)
        chunker = get_chunker(chunking_mode)

        # Process each text individually to handle long documents
        all_embeddings = []
        chunks_count = []
        all_chunk_spans = []

        for text_idx, text in enumerate(texts):
            logger.info(
                f"Processing text {text_idx + 1}/{len(texts)}, length: {len(text)}"
            )

            # Handle empty text
            if not text or not text.strip():
                logger.warning(f"Text {text_idx + 1} is empty, skipping")
                all_embeddings.append([])
                chunks_count.append(0)
                all_chunk_spans.append([])
                continue

            # For very long texts, we need to handle them in segments
            text_embeddings, text_chunks_count, text_chunk_spans = process_single_text(
                text, model, tokenizer, chunker, task, chunk_size, chunking_mode
            )

            all_embeddings.append(text_embeddings)
            chunks_count.append(text_chunks_count)
            all_chunk_spans.append(text_chunk_spans)

        logger.info("All embeddings generated successfully")
        return all_embeddings, chunks_count, all_chunk_spans

    except Exception as e:
        logger.error(f"Error generating embeddings: {str(e)}")
        import traceback

        logger.error(f"Full traceback: {traceback.format_exc()}")
        raise Exception(f"Embedding generation failed: {str(e)}")
