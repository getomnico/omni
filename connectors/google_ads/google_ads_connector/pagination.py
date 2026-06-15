"""Pagination helpers for Google Ads connector.

Google's official client handles Search pagination. This module is kept for
connector layout consistency and future page-token helpers.
"""

from collections.abc import Iterable
from typing import TypeVar

T = TypeVar("T")


def chunks(items: Iterable[T], size: int) -> Iterable[list[T]]:
    batch: list[T] = []
    for item in items:
        batch.append(item)
        if len(batch) >= size:
            yield batch
            batch = []
    if batch:
        yield batch
