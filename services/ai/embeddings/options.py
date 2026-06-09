"""Shared embedding request option handling."""

DEFAULT_CHUNK_SIZE = 512


def resolve_chunk_size(chunk_size: int | None, max_model_len: int | None = None) -> int:
    """Return a valid token chunk size, capped by the provider's model limit."""
    effective_chunk_size = chunk_size if chunk_size is not None else DEFAULT_CHUNK_SIZE
    if effective_chunk_size < 1:
        raise ValueError("chunk_size must be greater than 0")
    if max_model_len is not None:
        return min(effective_chunk_size, max_model_len)
    return effective_chunk_size
