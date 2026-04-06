ARG DEVICE=cpu

FROM python:3.12-slim AS base

# libgomp1: required by PyTorch CPU; libglib2.0-0: required by OpenCV headless
# g++: required by torch inductor for JIT C++ compilation
# curl: required for Docker healthcheck
RUN apt-get update && apt-get install -y --no-install-recommends \
        libgomp1 \
        libglib2.0-0 \
        g++ \
        curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY requirements.txt .

# --- CPU variant: pre-install CPU-only PyTorch (~200 MB vs ~4.7 GB with CUDA) ---
FROM base AS deps-cpu
RUN pip install --no-cache-dir torch torchvision --index-url https://download.pytorch.org/whl/cpu \
 && pip install --no-cache-dir -r requirements.txt \
 && pip uninstall -y opencv-python \
 && pip install --no-cache-dir --force-reinstall opencv-python-headless

# --- CUDA variant: install normally (includes CUDA runtime libs) ---
FROM base AS deps-cuda
# rapidocr (a docling dependency) pulls in opencv-python (non-headless).
# Replace it with the headless variant to avoid a missing libGL.so.1 error.
RUN pip install --no-cache-dir -r requirements.txt \
 && pip uninstall -y opencv-python \
 && pip install --no-cache-dir --force-reinstall opencv-python-headless

# --- Final stage: pick the variant based on DEVICE arg ---
FROM deps-${DEVICE}

COPY app.py .

EXPOSE 8003
CMD ["uvicorn", "app:app", "--host", "0.0.0.0", "--port", "8003"]
