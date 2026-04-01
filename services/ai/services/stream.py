"""Stream processing framework for LLM event streams.

Provides a base class for processors that transform an async stream of
LLM events into another async stream, enabling natural chaining.
"""

from abc import ABC, abstractmethod
from collections.abc import AsyncIterator

from anthropic.types.message_stream_event import MessageStreamEvent


class StreamProcessor(ABC):
    """Transforms an async stream of LLM events into another async stream.

    Processors are chained: each wraps the previous stream.
    A processor may buffer, transform, inject, or drop events.
    """

    @abstractmethod
    async def process(
        self, stream: AsyncIterator[MessageStreamEvent]
    ) -> AsyncIterator[MessageStreamEvent]:
        """Consume the input stream and yield transformed events.

        Implementations should iterate over `stream`, and yield zero or more
        output events per input event. Any buffered state should be flushed
        after the input stream is exhausted.
        """
        ...
