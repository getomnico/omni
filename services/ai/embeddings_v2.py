import os
import logging
import time
import torch
from transformers import AutoModel, AutoTokenizer
from transformers.tokenization_utils_base import BatchEncoding
import torch.nn.functional as F
import threading

logger = logging.getLogger(__name__)

MODEL_NAME = os.getenv("EMBEDDING_MODEL", "jinaai/jina-embeddings-v3")
QUERY_TASK = "retrieval.query"
PASSAGE_TASK = "retrieval.passage"
MAX_MODEL_SEQ_LEN = 8192
DEFAULT_TASK = PASSAGE_TASK

_model = None
_tokenizer = None
_model_lock = threading.Lock()


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


def tokenize(tokenizer: AutoTokenizer, texts: list[str]) -> BatchEncoding:
    start = time.time_ns()
    inputs = tokenizer(
        texts,
        return_tensors="pt",
        truncation=True,
        padding=True,
        return_offsets_mapping=True,
    )
    end = time.time_ns()
    print(
        f"Tokenized input: {inputs['input_ids'].shape} [took {(end - start) / 1e6:.2f} ms]"
    )
    return inputs


def forward(
    model: AutoModel, inputs: BatchEncoding, task="retrieval.passage"
) -> torch.Tensor:
    num_examples = inputs.input_ids.shape[0]
    task_id = model._adaptation_map[task]
    adapter_mask = torch.full(
        (num_examples,), task_id, dtype=torch.int32, device=model.device
    )

    inputs = {k: v.cuda() for k, v in inputs.items()}

    start = time.time_ns()
    with torch.no_grad():
        outputs = model.forward(**inputs, adapter_mask=adapter_mask, return_dict=True)
        embeddings = outputs.last_hidden_state
    end = time.time_ns()
    print(f"Embeddings: {embeddings.shape} [took {(end - start) / 1e6:.2f} ms]")
    return embeddings


type Span = tuple[int, int]
type SpanList = list[Span]
type BatchSpanList = list[SpanList]
type BatchCharSpanList = BatchSpanList
type BatchTokenSpanList = BatchSpanList


def chunk_by_sentences(
    inputs: BatchEncoding, tokenizer: AutoTokenizer, chunk_size: int = 512
) -> tuple[BatchCharSpanList, BatchTokenSpanList]:
    start = time.time_ns()
    batch_size = inputs.input_ids.shape[0]

    eos_indices = torch.where(inputs.input_ids == tokenizer.eos_token_id)[1]

    batch_chunk_char_spans = []
    batch_chunk_token_spans = []

    for i in range(batch_size):
        tokens = inputs.tokens(i)
        offset_mapping = inputs.offset_mapping[i]

        eos_idx = int(eos_indices[i].item())

        chunk_char_spans = []
        chunk_token_spans = []
        curr_sentence_start = 1
        curr_chunk_size = 0

        for j in range(1, eos_idx):
            if tokens[j] in [".", "!", "?"] or j == eos_idx - 1:
                start_token_span = offset_mapping[curr_sentence_start].tolist()
                end_token_span = offset_mapping[j].tolist()
                sentence_len = end_token_span[1] - start_token_span[0]
                if sentence_len >= 4:
                    # Found the next sentence
                    num_tokens_in_sentence = j - curr_sentence_start + 1
                    if len(chunk_char_spans) == 0:
                        # Each chunk should have at least one sentence, regardless of chunk size
                        chunk_token_spans.append((curr_sentence_start, j + 1))
                        chunk_char_spans.append(
                            (start_token_span[0], end_token_span[1] + 1)
                        )
                        curr_sentence_start = j + 1
                        curr_chunk_size = num_tokens_in_sentence
                    elif curr_chunk_size + num_tokens_in_sentence <= chunk_size:
                        # We can include this sentence in the curr chunk
                        chunk_token_spans[-1] = (chunk_token_spans[-1][0], j + 1)
                        chunk_char_spans[-1] = (
                            chunk_char_spans[-1][0],
                            end_token_span[1] + 1,
                        )
                        curr_sentence_start = j + 1
                        curr_chunk_size += num_tokens_in_sentence
                    else:
                        # Start a new chunk
                        chunk_token_spans.append((curr_sentence_start, j + 1))
                        chunk_char_spans.append(
                            (start_token_span[0], end_token_span[1] + 1)
                        )
                        curr_sentence_start = j + 1
                        curr_chunk_size = num_tokens_in_sentence

        batch_chunk_token_spans.append(chunk_token_spans)
        batch_chunk_char_spans.append(chunk_char_spans)

    end = time.time_ns()
    print(
        f"Num chunks: {[len(x) for x in batch_chunk_char_spans]} [took {(end - start) / 1e6:.2f} ms]"
    )
    return batch_chunk_char_spans, batch_chunk_token_spans


def apply_late_chunking(
    token_embeddings: torch.Tensor, chunk_spans: list[list[tuple[int, int]]]
):
    start = time.time_ns()

    # Extract all chunks in one go
    all_chunks = [
        token_embeddings[batch_idx, chunk_start:chunk_end, :].mean(dim=0)
        for batch_idx, spans in enumerate(chunk_spans)
        for chunk_start, chunk_end in spans
    ]

    # Stack and normalize all at once
    chunks = torch.stack(all_chunks, dim=0)
    norm_chunks = F.normalize(chunks, p=2, dim=1)  # dim=1 for feature dimension

    end = time.time_ns()
    print(f"Chunked embeddings: {len(norm_chunks)} [took {(end - start) / 1e6:.2f} ms]")
    return norm_chunks


class Chunk:
    def __init__(self, span: tuple[int, int], embedding: list[float]):
        self.span = span
        self.embedding = embedding


def generate_embeddings_sync(
    texts: list[str],
    task: str,
    chunk_size: int,
    chunking_mode: str,
    n_sentences: int = None,
) -> list[list[Chunk]]:
    try:
        logger.info(f"Starting embedding generation for {len(texts)} texts")
        model, tokenizer = load_model()
        logger.info("Model and tokenizer loaded successfully")

        tokens = tokenize(tokenizer, texts)

        if chunking_mode == "none":
            logger.info(f"Skipping chunking for embeddings input {texts}, task {task}.")
            embeddings = forward(model, tokens, task=task)  # (B, T, C) tensor
            embeddings = embeddings.mean(dim=1)  # Mean pooling
            embeddings = F.normalize(
                embeddings, p=2, dim=1
            ).tolist()  # Normalize to unit norm

            return [[Chunk((0, len(t)), embeddings[i])] for i, t in enumerate(texts)]

        # We will always use sentenced-based chunk-size limited chunking
        batch_chunk_char_spans, batch_chunk_token_spans = chunk_by_sentences(
            tokens, tokenizer, chunk_size=chunk_size
        )

        embeddings = forward(
            model, tokens, task=task
        )  # Embeddings is a (B, T, C) tensor

        # Chunk embeddings is a (N, C) tensor, where N is the total number of chunks
        # across all input texts
        all_chunk_embeddings = apply_late_chunking(embeddings, batch_chunk_token_spans)

        all_chunks = []
        chunk_idx = 0
        for i, text in enumerate(texts):
            num_chunks = len(batch_chunk_char_spans[i])
            chunk_embeddings = all_chunk_embeddings[
                chunk_idx : (chunk_idx + num_chunks), :
            ]
            chunks = [
                Chunk(batch_chunk_char_spans[i][chunk_idx], em)
                for chunk_idx, em in enumerate(chunk_embeddings.tolist())
            ]
            all_chunks.append(chunks)
            chunk_idx += num_chunks

        return all_chunks
    except Exception as e:
        logger.error(f"Error generating embeddings: {str(e)}")
        import traceback

        logger.error(f"Full traceback: {traceback.format_exc()}")
        raise Exception(f"Embedding generation failed: {str(e)}")
