FROM nvidia/cuda:12.1.0-devel-ubuntu22.04

# Set timezone and prevent interactive prompts
ENV DEBIAN_FRONTEND=noninteractive
ENV TZ=UTC

# Install Python 3.12
RUN apt-get update && apt-get install -y \
    software-properties-common \
    tzdata \
    && add-apt-repository ppa:deadsnakes/ppa \
    && apt-get update && apt-get install -y \
    python3.12 \
    python3.12-venv \
    python3.12-dev \
    gcc \
    g++ \
    ninja-build \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create symlinks for python
RUN ln -s /usr/bin/python3.12 /usr/bin/python

# Install pip using ensurepip
RUN python -m ensurepip --upgrade

# Install build dependencies first
RUN python -m pip install --upgrade pip setuptools wheel

WORKDIR /app

# Install flash-attn separately with build isolation disabled
RUN python -m pip install torch --index-url https://download.pytorch.org/whl/cu126
RUN python -m pip install --no-build-isolation flash-attn --no-cache-dir

# Install pip manually
RUN curl -sS https://bootstrap.pypa.io/get-pip.py | python

# Copy requirements and install Python dependencies
COPY requirements.txt .
RUN python -m pip install --no-cache-dir -r requirements.txt

# Copy application code
COPY . .

# Run the application
CMD sh -c "uvicorn main:app --host 0.0.0.0 --port ${PORT}"
