# Stage 1: Builder - compile wheels
FROM python:3.11-slim AS builder
WORKDIR /build
RUN pip install --no-cache-dir hatchling

# Copy and build SDK
COPY sdk/python /sdk/python
RUN pip wheel --no-deps -w /wheels /sdk/python

# Copy and build connector
COPY connectors/telegram /build
RUN pip wheel --no-deps -w /wheels /build

# Stage 2: Runtime
FROM python:3.11-slim
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /wheels /wheels
RUN pip install --no-cache-dir /wheels/*.whl && rm -rf /wheels

COPY connectors/telegram/main.py /app/

ENV PYTHONUNBUFFERED=1
CMD ["python", "main.py"]
