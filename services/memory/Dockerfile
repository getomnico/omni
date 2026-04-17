FROM python:3.12-slim

ENV PYTHONDONTWRITEBYTECODE=1
ENV PYTHONUNBUFFERED=1

WORKDIR /app

RUN apt-get update && apt-get install -y curl && rm -rf /var/lib/apt/lists/*

# mem0ai 2.0.0 no longer ships a server — we run our own minimal FastAPI wrapper.
# psycopg[binary] is needed for the pgvector store backend.
RUN pip install --no-cache-dir \
    "mem0ai>=2.0.0" \
    "fastapi>=0.115" \
    "uvicorn[standard]>=0.34" \
    "psycopg[binary]>=3.2" \
    "psycopg-pool>=3.2,<4" \
    httpx \
    "opentelemetry-sdk>=1.20" \
    "opentelemetry-exporter-otlp-proto-http>=1.20" \
    "opentelemetry-instrumentation-fastapi>=0.41b0" \
    "opentelemetry-instrumentation-httpx>=0.41b0"

COPY server.py /app/server.py
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 8888

ENTRYPOINT ["/entrypoint.sh"]
