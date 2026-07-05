"""Streaming subpackage — decoupled producer/consumer chat streaming.

Extracted from the ``services/ai/routers/chat.py`` monolith into three layers:

* ``persist`` — SSE event helpers, typed event types, persistence wrapper
* ``run`` — Redis transport layer, producer/consumer/lifecycle
* ``generate`` — Agent loop generator (the core stream generator)
"""
