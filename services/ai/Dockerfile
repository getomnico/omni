FROM python:3.12-slim

ENV PYTHONDONTWRITEBYTECODE=1
ENV PYTHONUNBUFFERED=1

# Install system dependencies
RUN apt-get update && apt-get install -y \
    gcc \
    g++ \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy requirements and create CPU-only version
COPY requirements.txt requirements-original.txt

# Create CPU-only requirements by removing GPU-specific packages
RUN grep -v -E "(flash-attn|nvidia-|triton)" requirements-original.txt > requirements-cpu.txt

# Install CPU-only PyTorch (much smaller - ~200MB vs 3GB+)
RUN pip install --no-cache-dir torch torchvision --index-url https://download.pytorch.org/whl/cpu

# Install remaining dependencies
RUN pip install --no-cache-dir -r requirements-cpu.txt

# Copy application code
COPY . .

# Run the application
# Use AI_WORKERS environment variable for number of workers (defaults to 1)
CMD sh -c "python -m uvicorn main:app --host 0.0.0.0 --port 8000 --workers ${AI_WORKERS:-1}"