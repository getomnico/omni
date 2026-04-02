FROM python:3.12-slim


# libgomp1: required by PyTorch CPU; libglib2.0-0: required by OpenCV headless
# g++: required by torch inductor for JIT C++ compilation
RUN apt-get update && apt-get install -y --no-install-recommends \
        libgomp1 \
        libglib2.0-0 \
        g++ \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY requirements.txt .

# rapidocr (a docling dependency) pulls in opencv-python (non-headless).
# Replace it with the headless variant to avoid a missing libGL.so.1 error.
RUN pip install --no-cache-dir -r requirements.txt \
 && pip uninstall -y opencv-python \
 && pip install --no-cache-dir --force-reinstall opencv-python-headless

COPY app.py .

EXPOSE 8003
CMD ["uvicorn", "app:app", "--host", "0.0.0.0", "--port", "8003"]
