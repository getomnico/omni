# Docling Service

REST API service that converts documents to Markdown using [Docling](https://github.com/docling-project/docling).

## Supported formats

PDF, DOCX, XLSX, PPTX, HTML, images (PNG, JPEG, TIFF, BMP, WEBP), and more — see [Docling supported formats](https://docling-project.github.io/docling/usage/supported_formats/).

## Configuration

This service is **opt-in** and disabled by default. To enable:

1. **Via Environment Variable**: Set `DOCLING_ENABLED=true` in your `.env` file
2. **Via Admin UI**: Navigate to Admin Settings → Document Conversion and toggle the setting

When enabled, all document conversions across all connectors will use Docling instead of the built-in lightweight extractors.

## Advantages over built-in extraction

- **Superior PDF extraction** — AI-based layout analysis correctly handles tables, multi-column layouts, headers/footers, and reading order
- **Built-in OCR** — Supports scanned PDFs and image files that would otherwise produce no indexable content
- **Structure-aware output** — Preserves headings, sections, and table structure for better downstream chunking

## Trade-offs

- **Slower processing** — AI-based pipeline is significantly slower than lightweight libraries for simple documents
- **Larger footprint** — Requires ML model downloads (~400 MB cached in Docker volume)
- **GPU recommended** — CPU-only mode works but is slow; GPU acceleration is recommended for production but currently not supported

## API

Conversion is asynchronous. Submit a file and get a job ID back immediately, then poll for the result.

**`POST /convert`** — `multipart/form-data`, field `file`. The filename extension is used for format detection. Returns `202 Accepted` with `{"job_id": "<uuid>"}`.

**`GET /jobs/{job_id}`** — Poll a job. Response body always contains `status`:

| `status` | Additional fields | Meaning |
|---|---|---|
| `pending` | — | Queued, waiting for a free slot |
| `running` | — | Actively converting |
| `completed` | `markdown` | Conversion succeeded; `markdown` contains the result |
| `failed` | `detail` | Conversion failed; `detail` explains why |

`404` is returned if the job ID is unknown.

**`GET /health`** — Returns `{"status": "ok"}` once the service is ready.

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `MAX_CONCURRENT_CONVERSIONS` | `1` | Maximum simultaneous conversions |

## Running

Start the service via Docker Compose profile from the project root:

```bash
cd docker
docker compose --profile docling up -d
```

On first start the service downloads the required models (~400 MB) and caches them in a named Docker volume.

Interactive docs at `http://localhost:8000/docs`.
