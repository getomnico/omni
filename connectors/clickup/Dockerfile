# ClickUp Connector Dockerfile

FROM python:3.11-slim as builder

WORKDIR /build

# Install build dependencies
RUN pip install --no-cache-dir hatchling

# Copy and build the SDK
COPY sdk/python /sdk/python
RUN pip wheel --no-deps -w /wheels /sdk/python

# Copy and build the connector
COPY connectors/clickup /build
RUN pip wheel --no-deps -w /wheels /build

# Production stage
FROM python:3.11-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy wheels and install
COPY --from=builder /wheels /wheels
RUN pip install --no-cache-dir /wheels/*.whl && rm -rf /wheels

# Copy entry point
COPY connectors/clickup/main.py /app/

ENV PYTHONUNBUFFERED=1

CMD ["python", "main.py"]
