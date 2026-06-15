FROM python:3.11-slim as builder

WORKDIR /build

RUN pip install --no-cache-dir hatchling

COPY sdk/python /sdk/python
RUN pip wheel --no-deps -w /wheels /sdk/python

COPY connectors/google_ads /build
RUN pip wheel --no-deps -w /wheels /build

FROM python:3.11-slim

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /wheels /wheels
RUN pip install --no-cache-dir /wheels/*.whl && rm -rf /wheels

COPY connectors/google_ads/main.py /app/

ENV PYTHONUNBUFFERED=1

CMD ["python", "main.py"]
