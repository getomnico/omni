"""Stream processing framework for LLM event streams.

Provides a base class for processors that can transform, buffer, or inject
events in a streaming LLM response pipeline.
"""

from abc import ABC, abstractmethod

from anthropic.types.message_stream_event import MessageStreamEvent


class StreamProcessor(ABC):
    """Processes streaming LLM events, potentially transforming, buffering, or injecting events.

    Processors are chained: each event passes through every processor in order.
    A processor may return zero events (buffering/swallowing), one event (passthrough
    or transformation), or multiple events (injection).
    """

    @abstractmethod
    def process(self, event: MessageStreamEvent) -> list[MessageStreamEvent]:
        """Process one event, returning zero or more output events."""
        ...

    def flush(self) -> list[MessageStreamEvent]:
        """Emit any remaining buffered events. Called at end of stream."""
        return []
