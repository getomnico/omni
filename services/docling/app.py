"""Document to Markdown conversion service using Docling."""

import asyncio
import io
import logging
import multiprocessing
import os
import time
import uuid
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from enum import Enum

import uvicorn
from docling.datamodel.accelerator_options import AcceleratorDevice, AcceleratorOptions
from docling.datamodel.base_models import ConversionStatus, InputFormat
from docling.datamodel.document import DocumentStream
from docling.datamodel.pipeline_options import (
    PdfPipelineOptions,
    RapidOcrOptions,
    TableFormerMode,
)
from docling.document_converter import DocumentConverter, PdfFormatOption
from fastapi import FastAPI, File, HTTPException, UploadFile
from fastapi.responses import JSONResponse

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

_MAX_CONCURRENT = int(os.getenv("MAX_CONCURRENT_CONVERSIONS", "1"))
_converter_pool: asyncio.Queue[DocumentConverter]  # created inside lifespan
_ready: bool = False


class _SuppressFilter(logging.Filter):
    """Drop noisy RapidOCR messages that fire on blank/whitespace page regions."""

    _SUPPRESSED = {
        "The text detection result is empty",
        "RapidOCR returned empty result!",
    }

    def filter(self, record: logging.LogRecord) -> bool:
        return not any(s in record.getMessage() for s in self._SUPPRESSED)


def _apply_rapidocr_suppression() -> None:
    """Must be called after DocumentConverter init has triggered the rapidocr import."""
    for name in ("RapidOCR", "docling.models.stages.ocr.rapid_ocr_model"):
        lg = logging.getLogger(name)
        lg.setLevel(logging.ERROR)
        f = _SuppressFilter()
        if not any(isinstance(x, _SuppressFilter) for x in lg.filters):
            lg.addFilter(f)
        for handler in lg.handlers:
            if not any(isinstance(x, _SuppressFilter) for x in handler.filters):
                handler.addFilter(f)


def _build_converter() -> DocumentConverter:
    """Build a DocumentConverter with optimal quality settings for CPU inference."""
    accelerator_options = AcceleratorOptions(
        device=AcceleratorDevice.CPU,
        num_threads=multiprocessing.cpu_count(),
    )
    pipeline_options = PdfPipelineOptions()
    pipeline_options.accelerator_options = accelerator_options
    pipeline_options.do_table_structure = True
    pipeline_options.table_structure_options.mode = TableFormerMode.ACCURATE
    pipeline_options.do_ocr = True
    pipeline_options.ocr_options = RapidOcrOptions(backend="torch")
    pipeline_options.images_scale = (
        1.5  # higher resolution improves OCR/layout accuracy
    )
    pipeline_options.generate_picture_images = True
    pipeline_options.generate_table_images = True
    pipeline_options.do_code_enrichment = (
        False  # VLM per code block; adds latency on code-heavy docs
    )
    pipeline_options.do_formula_enrichment = (
        False  # VLM per formula; adds latency on math-heavy docs
    )
    pipeline_options.do_picture_classification = (
        True  # lightweight ViT; minimal overhead
    )
    pipeline_options.do_picture_description = (
        False  # SmolVLM per image; significant latency per figure
    )
    pipeline_options.do_chart_extraction = (
        False  # Granite Vision 2B per chart; high RAM + latency
    )
    converter = DocumentConverter(
        format_options={
            InputFormat.PDF: PdfFormatOption(pipeline_options=pipeline_options),
        }
    )
    _apply_rapidocr_suppression()
    return converter


def _download_models() -> None:
    """Pre-download all Docling models so the first conversion doesn't stall."""
    from docling.utils.model_downloader import download_models

    download_models(
        progress=True,
        with_layout=True,
        with_tableformer=True,
        with_code_formula=False,  # ~500 MB; needed for do_code/formula_enrichment
        with_picture_classifier=True,  # ~90 MB; needed for do_picture_classification
        with_smolvlm=False,  # ~500 MB; needed for do_picture_description
        with_granitedocling=False,
        with_granitedocling_mlx=False,
        with_smoldocling=False,
        with_smoldocling_mlx=False,
        with_granite_vision=False,
        with_granite_chart_extraction=False,  # ~2 GB; needed for do_chart_extraction
        with_rapidocr=True,
        with_easyocr=False,
    )


class JobStatus(str, Enum):
    PENDING = "pending"
    RUNNING = "running"
    COMPLETED = "completed"
    FAILED = "failed"


@dataclass
class Job:
    id: str
    status: JobStatus = JobStatus.PENDING
    markdown: str | None = None
    detail: str | None = None
    created_at: datetime = field(default_factory=lambda: datetime.now(timezone.utc))


_jobs: dict[str, Job] = {}


@asynccontextmanager
async def lifespan(app: FastAPI):
    global _converter_pool
    _converter_pool = asyncio.Queue(maxsize=_MAX_CONCURRENT)
    # Start converter init in the background so the HTTP server becomes
    # responsive immediately. /health returns {"status": "starting"} and
    # /convert returns 503 until init completes.
    asyncio.create_task(_init_converter())
    asyncio.create_task(_cleanup_loop())
    yield


async def _init_converter() -> None:
    global _ready
    try:
        logger.info(
            "Loading %d DocumentConverter instance(s) ...",
            _MAX_CONCURRENT,
        )
        logger.info("Downloading models ...")
        loop = asyncio.get_running_loop()
        await loop.run_in_executor(None, _download_models)
        for i in range(_MAX_CONCURRENT):
            converter = await loop.run_in_executor(None, _build_converter)
            _converter_pool.put_nowait(converter)
            logger.info("Converter %d/%d ready.", i + 1, _MAX_CONCURRENT)
        # All models are loaded; disable network validation for all subsequent HF hub calls.
        os.environ["HF_HUB_OFFLINE"] = "1"
        _ready = True
        logger.info("Ready — %d converter(s) available.", _MAX_CONCURRENT)
    except Exception:
        logger.exception(
            "Failed to initialise converter(s). Service will remain unavailable."
        )


async def _cleanup_loop() -> None:
    """Hourly sweep: delete jobs older than 1 day regardless of status."""
    while True:
        await asyncio.sleep(3600)
        cutoff = datetime.now(timezone.utc) - timedelta(days=1)
        stale = [jid for jid, j in _jobs.items() if j.created_at < cutoff]
        for jid in stale:
            del _jobs[jid]
        if stale:
            logger.info("Cleaned up %d stale job(s).", len(stale))


async def _run_job(job: Job, data: bytes, filename: str) -> None:
    """Background task: lease a converter from the pool, run conversion, return it."""
    converter = await _converter_pool.get()
    try:
        job.status = JobStatus.RUNNING
        logger.info(
            "Job %s: conversion started (%s, %d bytes).", job.id, filename, len(data)
        )
        t0 = time.monotonic()
        stream = DocumentStream(name=filename, stream=io.BytesIO(data))
        try:
            result = await asyncio.get_running_loop().run_in_executor(
                None, lambda: converter.convert(stream)
            )
        except Exception as exc:
            elapsed = time.monotonic() - t0
            job.status = JobStatus.FAILED
            if (
                "not supported" in str(exc).lower()
                or "cannot convert" in str(exc).lower()
            ):
                job.detail = f"Unsupported format: {exc}"
            else:
                job.detail = f"Conversion error: {exc}"
            logger.error(
                "Job %s: failed after %.1fs — %s",
                job.id,
                elapsed,
                job.detail,
                exc_info=True,
            )
            return
    finally:
        _converter_pool.put_nowait(converter)

    elapsed = time.monotonic() - t0
    # Converter returned to pool; do cheap post-processing outside the slot.
    try:
        if result.status not in (
            ConversionStatus.SUCCESS,
            ConversionStatus.PARTIAL_SUCCESS,
        ):
            job.status = JobStatus.FAILED
            job.detail = "Conversion failed."
            logger.error("Job %s: failed after %.1fs — %s", job.id, elapsed, job.detail)
            return
        job.markdown = result.document.export_to_markdown()
        job.status = JobStatus.COMPLETED
        logger.info(
            "Job %s: completed in %.1fs (%d chars).", job.id, elapsed, len(job.markdown)
        )
    except Exception as exc:
        job.status = JobStatus.FAILED
        job.detail = f"Post-processing error: {exc}"
        logger.error(
            "Job %s: failed after %.1fs — %s",
            job.id,
            elapsed,
            job.detail,
            exc_info=True,
        )


app = FastAPI(title="docling", version="1.0.0", lifespan=lifespan)


@app.get("/health")
def health():
    if _ready:
        return {"status": "ok"}
    return JSONResponse(status_code=503, content={"status": "starting"})


@app.post("/convert", status_code=202)
async def submit_conversion(file: UploadFile = File(...)):
    """Submit a document for conversion. Returns a job ID immediately (HTTP 202)."""
    if not _ready:
        raise HTTPException(
            status_code=503,
            detail="Service is starting up; models are being loaded. Try again shortly.",
        )
    if not file.filename:
        raise HTTPException(
            status_code=400, detail="A filename with extension is required."
        )

    data = await file.read()
    job_id = str(uuid.uuid4())
    job = Job(id=job_id)
    _jobs[job_id] = job
    asyncio.create_task(_run_job(job, data, file.filename))
    logger.info("Job %s: submitted (%s, %d bytes).", job_id, file.filename, len(data))
    return {"job_id": job_id}


@app.get("/jobs/{job_id}")
async def get_job(job_id: str):
    """Poll a conversion job. Returns status and, when complete, the Markdown result."""
    job = _jobs.get(job_id)
    if job is None:
        raise HTTPException(status_code=404, detail=f"Unknown job ID: {job_id!r}")
    if job.status == JobStatus.COMPLETED:
        logger.info("Job %s: result retrieved.", job_id)
        del _jobs[job_id]
        return {"status": job.status, "markdown": job.markdown}
    if job.status == JobStatus.FAILED:
        logger.info("Job %s: failure retrieved.", job_id)
        del _jobs[job_id]
        return {"status": job.status, "detail": job.detail}
    logger.info("Job %s: polled — %s.", job_id, job.status.value)
    return {"status": job.status}  # pending or running


if __name__ == "__main__":
    port_str = os.getenv("PORT")
    if not port_str:
        logger.error("PORT environment variable is required but not set")
        raise SystemExit(1)
    uvicorn.run("app:app", host="0.0.0.0", port=int(port_str), log_level="info")
