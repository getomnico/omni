FROM python:3.11-slim

WORKDIR /app
COPY sdk/python /sdk/python
COPY connectors/snowflake /app
RUN pip install --no-cache-dir /sdk/python[mcp] .

ENV PYTHONUNBUFFERED=1
CMD ["python", "main.py"]
