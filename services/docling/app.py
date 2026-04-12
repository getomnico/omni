"""Document to Markdown conversion service using Docling."""

import asyncio
import gc
import io
import logging
import os
import time
import uuid
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from enum import Enum
from typing import Literal

import uvicorn
from docling.datamodel.accelerator_options import AcceleratorOptions
from docling.datamodel.base_models import ConversionStatus, InputFormat
from docling.datamodel.document import DocumentStream
from docling.datamodel.pipeline_options import (
    PdfPipelineOptions,
    RapidOcrOptions,
    TableFormerMode,
)
from docling.document_converter import DocumentConverter, PdfFormatOption
from fastapi import FastAPI, File, HTTPException, Query, UploadFile
from fastapi.responses import JSONResponse

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

_MAX_CONCURRENT = int(os.getenv("MAX_CONCURRENT_CONVERSIONS", "1"))
_MAX_PENDING = int(os.getenv("MAX_PENDING_JOBS", str(_MAX_CONCURRENT * 2)))
_ready: bool = False

PresetName = Literal["fast", "balanced", "quality"]
VALID_PRESETS: set[str] = {"fast", "balanced", "quality"}

QUALITY_PRESETS: dict[str, dict] = {
    "fast": {
        "table_structure_mode": TableFormerMode.FAST,
        "images_scale": 1.0,
        "do_picture_classification": False,
        "generate_picture_images": False,
        "generate_table_images": False,
    },
    "balanced": {
        "table_structure_mode": TableFormerMode.ACCURATE,
        "images_scale": 1.0,
        "do_picture_classification": True,
        "generate_picture_images": False,
        "generate_table_images": False,
    },
    "quality": {
        "table_structure_mode": TableFormerMode.ACCURATE,
        "images_scale": 1.5,
        "do_picture_classification": True,
        "generate_picture_images": True,
        "generate_table_images": True,
    },
}

# Semaphore to limit concurrent conversions (replaces converter pool).
# A fresh converter is built per job and destroyed after to avoid memory accumulation.
_semaphore: asyncio.Semaphore  # created in lifespan


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


def _build_converter(preset: str = "balanced") -> DocumentConverter:
    """Build a DocumentConverter with pipeline options from the given quality preset."""
    opts = QUALITY_PRESETS[preset]
    accelerator_options = AcceleratorOptions()
    pipeline_options = PdfPipelineOptions()
    pipeline_options.accelerator_options = accelerator_options
    pipeline_options.do_table_structure = True
    pipeline_options.table_structure_options.mode = opts["table_structure_mode"]
    pipeline_options.do_ocr = True
    pipeline_options.ocr_options = RapidOcrOptions(backend="torch")
    pipeline_options.images_scale = opts["images_scale"]
    pipeline_options.generate_picture_images = opts["generate_picture_images"]
    pipeline_options.generate_table_images = opts["generate_table_images"]
    pipeline_options.do_code_enrichment = False
    pipeline_options.do_formula_enrichment = False
    pipeline_options.do_picture_classification = opts["do_picture_classification"]
    pipeline_options.do_picture_description = False
    pipeline_options.do_chart_extraction = False
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
        with_code_formula=False,
        with_picture_classifier=True,
        with_smolvlm=False,
        with_granitedocling=False,
        with_granitedocling_mlx=False,
        with_smoldocling=False,
        with_smoldocling_mlx=False,
        with_granite_vision=False,
        with_granite_chart_extraction=False,
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
    preset: str = "balanced"
    status: JobStatus = JobStatus.PENDING
    markdown: str | None = None
    detail: str | None = None
    created_at: datetime = field(default_factory=lambda: datetime.now(timezone.utc))


_jobs: dict[str, Job] = {}


def _active_job_count() -> int:
    """Count jobs that are pending or running (holding memory)."""
    return sum(
        1 for j in _jobs.values() if j.status in (JobStatus.PENDING, JobStatus.RUNNING)
    )


@asynccontextmanager
async def lifespan(app: FastAPI):
    global _semaphore
    _semaphore = asyncio.Semaphore(_MAX_CONCURRENT)
    # Start init in the background so the HTTP server becomes responsive
    # immediately. /health returns {"status": "starting"} and /convert
    # returns 503 until init completes.
    asyncio.create_task(_init())
    asyncio.create_task(_cleanup_loop())
    yield


async def _init() -> None:
    global _ready
    try:
        logger.info("Downloading models ...")
        loop = asyncio.get_running_loop()
        await loop.run_in_executor(None, _download_models)
        # Warm up: build and immediately discard one converter so all model
        # weights are loaded into the HF cache and first real job is fast.
        await loop.run_in_executor(None, _build_converter, "balanced")
        os.environ["HF_HUB_OFFLINE"] = "1"
        _ready = True
        logger.info(
            "Ready — models cached, max %d concurrent conversion(s).", _MAX_CONCURRENT
        )
    except Exception:
        logger.exception("Failed to initialise. Service will remain unavailable.")


async def _cleanup_loop() -> None:
    """Periodic sweep: remove finished jobs after 10 minutes, any job after 1 day."""
    while True:
        await asyncio.sleep(60)
        now = datetime.now(timezone.utc)
        finished_cutoff = now - timedelta(minutes=10)
        absolute_cutoff = now - timedelta(days=1)
        stale = [
            jid
            for jid, j in _jobs.items()
            if j.created_at < absolute_cutoff
            or (
                j.status in (JobStatus.COMPLETED, JobStatus.FAILED)
                and j.created_at < finished_cutoff
            )
        ]
        for jid in stale:
            del _jobs[jid]
        if stale:
            logger.info("Cleaned up %d stale job(s).", len(stale))


async def _run_job(job: Job, data: bytes, filename: str) -> None:
    """Background task: build a fresh converter, run conversion, destroy everything."""
    await _semaphore.acquire()
    result = None
    converter = None
    try:
        job.status = JobStatus.RUNNING
        logger.info(
            "Job %s: conversion started (%s, %d bytes, preset=%s).",
            job.id,
            filename,
            len(data),
            job.preset,
        )
        t0 = time.monotonic()
        loop = asyncio.get_running_loop()
        converter = await loop.run_in_executor(None, _build_converter, job.preset)
        buf = io.BytesIO(data)
        del data
        stream = DocumentStream(name=filename, stream=buf)
        try:
            result = await loop.run_in_executor(None, lambda: converter.convert(stream))
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
            buf.close()

        elapsed = time.monotonic() - t0
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
        if job.status not in (JobStatus.COMPLETED, JobStatus.FAILED):
            job.status = JobStatus.FAILED
            job.detail = f"Unexpected error: {exc}"
            logger.error("Job %s: failed — %s", job.id, job.detail, exc_info=True)
    finally:
        del result, converter
        gc.collect()
        _semaphore.release()


app = FastAPI(title="docling", version="1.0.0", lifespan=lifespan)


@app.get("/health")
def health():
    if _ready:
        return {"status": "ok"}
    return JSONResponse(status_code=503, content={"status": "starting"})


@app.get("/queue-status")
def queue_status():
    pending = sum(1 for j in _jobs.values() if j.status == JobStatus.PENDING)
    running = sum(1 for j in _jobs.values() if j.status == JobStatus.RUNNING)
    return {"pending": pending, "running": running, "max": _MAX_PENDING}


@app.get("/presets")
def list_presets():
    """Return available quality presets and their configurations."""
    return {"presets": list(QUALITY_PRESETS.keys())}


@app.post("/convert", status_code=202)
async def submit_conversion(
    file: UploadFile = File(...),
    preset: str = Query("balanced"),
):
    """Submit a document for conversion. Returns a job ID immediately (HTTP 202)."""
    if not _ready:
        raise HTTPException(
            status_code=503,
            detail="Service is starting up; models are being loaded. Try again shortly.",
        )
    if preset not in VALID_PRESETS:
        raise HTTPException(
            status_code=400,
            detail=f"Invalid preset '{preset}'. Must be one of: {', '.join(sorted(VALID_PRESETS))}",
        )
    if _active_job_count() >= _MAX_PENDING:
        return JSONResponse(
            status_code=429,
            content={"detail": "Too many pending conversions. Try again later."},
            headers={"Retry-After": "30"},
        )

    if not file.filename:
        raise HTTPException(
            status_code=400, detail="A filename with extension is required."
        )

    data = await file.read()
    job_id = str(uuid.uuid4())
    job = Job(id=job_id, preset=preset)
    _jobs[job_id] = job
    asyncio.create_task(_run_job(job, data, file.filename))
    logger.info(
        "Job %s: submitted (%s, %d bytes, preset=%s).",
        job_id,
        file.filename,
        len(data),
        preset,
    )
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
