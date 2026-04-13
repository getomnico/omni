"""Document to Markdown conversion service using Docling."""

import asyncio
import gc
import io
import logging
import os
import time
import uuid
from concurrent.futures import ThreadPoolExecutor
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from enum import Enum

import pypdfium2 as pdfium
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
_PAGES_PER_CHUNK = int(os.getenv("PAGES_PER_CHUNK", "10"))
_MAX_JOB_SECONDS = int(os.getenv("MAX_JOB_SECONDS", "1800"))
_CONVERTER_RECYCLE_AFTER = int(os.getenv("CONVERTER_RECYCLE_AFTER", "20"))

VALID_PRESETS: frozenset[str] = frozenset({"fast", "balanced", "quality"})

# Preset pipeline options. Heavy/unused options stay off everywhere:
# picture/table image generation (we only emit markdown), picture
# classification (not reflected in markdown), picture description (VLM),
# chart extraction (large model, narrow benefit).
QUALITY_PRESETS: dict[str, dict] = {
    "fast": {
        "do_ocr": False,
        "table_structure_mode": TableFormerMode.FAST,
        "images_scale": 1.0,
        "do_code_enrichment": False,
        "do_formula_enrichment": False,
    },
    "balanced": {
        "do_ocr": False,
        "table_structure_mode": TableFormerMode.ACCURATE,
        "images_scale": 1.0,
        "do_code_enrichment": False,
        "do_formula_enrichment": False,
    },
    "quality": {
        "do_ocr": True,
        "table_structure_mode": TableFormerMode.ACCURATE,
        "images_scale": 1.5,
        "do_code_enrichment": True,
        "do_formula_enrichment": True,
    },
}


class _SuppressFilter(logging.Filter):
    """Drop noisy RapidOCR messages that fire on blank/whitespace page regions."""

    _SUPPRESSED = {
        "The text detection result is empty",
        "RapidOCR returned empty result!",
    }

    def filter(self, record: logging.LogRecord) -> bool:
        return not any(s in record.getMessage() for s in self._SUPPRESSED)


def _apply_rapidocr_suppression() -> None:
    for name in ("RapidOCR", "docling.models.stages.ocr.rapid_ocr_model"):
        lg = logging.getLogger(name)
        lg.setLevel(logging.ERROR)
        if not any(isinstance(x, _SuppressFilter) for x in lg.filters):
            lg.addFilter(_SuppressFilter())
        for handler in lg.handlers:
            if not any(isinstance(x, _SuppressFilter) for x in handler.filters):
                handler.addFilter(_SuppressFilter())


def _build_converter(preset: str) -> DocumentConverter:
    opts = QUALITY_PRESETS[preset]
    pipeline_options = PdfPipelineOptions()
    pipeline_options.accelerator_options = AcceleratorOptions()
    # Docling's cooperative timeout: on expiry it stops feeding pages and
    # returns PARTIAL_SUCCESS — no zombie threads, no process kill needed.
    pipeline_options.document_timeout = float(_MAX_JOB_SECONDS)
    pipeline_options.do_table_structure = True
    pipeline_options.table_structure_options.mode = opts["table_structure_mode"]
    pipeline_options.do_ocr = opts["do_ocr"]
    if opts["do_ocr"]:
        pipeline_options.ocr_options = RapidOcrOptions(backend="torch")
    pipeline_options.images_scale = opts["images_scale"]
    pipeline_options.generate_picture_images = False
    pipeline_options.generate_table_images = False
    pipeline_options.do_picture_classification = False
    pipeline_options.do_picture_description = False
    pipeline_options.do_chart_extraction = False
    pipeline_options.do_code_enrichment = opts["do_code_enrichment"]
    pipeline_options.do_formula_enrichment = opts["do_formula_enrichment"]
    converter = DocumentConverter(
        format_options={
            InputFormat.PDF: PdfFormatOption(pipeline_options=pipeline_options),
        }
    )
    if opts["do_ocr"]:
        _apply_rapidocr_suppression()
    return converter


@dataclass
class PresetConverter:
    """Shared converter for a preset with a lock (serializes concurrent use
    when _MAX_CONCURRENT > 1) and a job counter driving periodic recycling
    to reclaim ONNX/torch residual memory."""

    preset: str
    converter: DocumentConverter
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)
    job_count: int = 0

    async def maybe_recycle(self) -> None:
        if _CONVERTER_RECYCLE_AFTER <= 0 or self.job_count < _CONVERTER_RECYCLE_AFTER:
            return
        logger.info(
            "Recycling converter [%s] after %d jobs.", self.preset, self.job_count
        )
        loop = asyncio.get_running_loop()
        # Build the replacement off the event loop and off the conversion
        # pool so we don't starve other work.
        new_converter = await loop.run_in_executor(
            _build_executor, _build_converter, self.preset
        )
        old = self.converter
        self.converter = new_converter
        self.job_count = 0
        del old
        gc.collect()


_converters: dict[str, PresetConverter] = {}
_failed_presets: set[str] = set()
_semaphore: asyncio.Semaphore  # created in lifespan
_ready: bool = False

# Dedicated fixed-size pool for conversions — exactly _MAX_CONCURRENT workers,
# so we don't rely on asyncio's default executor (sized to min(32, cpu+4)).
# Separate tiny pool for converter builds (init + recycling) so recycling
# never competes with in-flight conversions for a worker thread.
_conversion_executor: ThreadPoolExecutor  # created in lifespan
_build_executor: ThreadPoolExecutor  # created in lifespan


def _cleanup_result(result: object) -> None:
    """Release backend resources held by a ConversionResult.

    Docling backends (pypdfium2, docling-parse) retain parsed page data and
    PDF objects in memory until explicitly unloaded.
    See: https://github.com/docling-project/docling/issues/2209
    """
    try:
        if hasattr(result, "input") and hasattr(result.input, "_backend"):
            backend = result.input._backend
            if backend is not None:
                backend.unload()
        if hasattr(result, "pages"):
            for page in result.pages:
                if hasattr(page, "_backend") and page._backend is not None:
                    page._backend.unload()
                    page._backend = None
                page._image_cache = {}
    except Exception:
        logger.debug("Error during result cleanup", exc_info=True)
    finally:
        del result
        gc.collect()


def _split_pdf(data: bytes, chunk_size: int) -> list[bytes] | None:
    """Split PDF bytes into per-chunk PDF byte buffers using pypdfium2.

    Returns None if the input is not a PDF. Each chunk is a standalone PDF
    containing at most `chunk_size` pages, so downstream conversion avoids
    re-parsing the full file for every chunk.
    """
    try:
        src = pdfium.PdfDocument(data)
    except Exception:
        return None
    try:
        page_count = len(src)
        if page_count <= chunk_size:
            return [data]
        chunks: list[bytes] = []
        for start in range(0, page_count, chunk_size):
            end = min(start + chunk_size, page_count)
            dst = pdfium.PdfDocument.new()
            try:
                dst.import_pages(src, list(range(start, end)))
                buf = io.BytesIO()
                dst.save(buf)
                chunks.append(buf.getvalue())
            finally:
                dst.close()
        return chunks
    finally:
        src.close()


def _convert_document(converter: DocumentConverter, data: bytes, filename: str) -> str:
    """Convert a document to markdown. PDFs are split into page chunks up
    front so each chunk is a small standalone PDF (bounded peak memory, no
    repeated full-file parses).

    Enforces a whole-job wall-clock deadline across chunks: once exceeded,
    the loop stops and returns whatever markdown was already produced. The
    docling-internal `document_timeout` provides per-chunk cooperative
    cancellation within this budget.
    """
    chunks = _split_pdf(data, _PAGES_PER_CHUNK)

    # Non-PDF: single-pass conversion.
    if chunks is None:
        stream = DocumentStream(name=filename, stream=io.BytesIO(data))
        result = converter.convert(stream)
        try:
            if result.status not in (
                ConversionStatus.SUCCESS,
                ConversionStatus.PARTIAL_SUCCESS,
            ):
                raise RuntimeError("Conversion failed.")
            return result.document.export_to_markdown()
        finally:
            _cleanup_result(result)

    markdown_parts: list[str] = []
    total = len(chunks)
    deadline = time.monotonic() + _MAX_JOB_SECONDS
    for idx, chunk in enumerate(chunks, start=1):
        if time.monotonic() >= deadline:
            logger.warning(
                "Job deadline reached after chunk %d/%d; stopping.", idx - 1, total
            )
            break
        logger.debug("Processing chunk %d/%d (%d bytes)", idx, total, len(chunk))
        stream = DocumentStream(name=filename, stream=io.BytesIO(chunk))
        result = converter.convert(stream)
        try:
            if result.status in (
                ConversionStatus.SUCCESS,
                ConversionStatus.PARTIAL_SUCCESS,
            ):
                markdown_parts.append(result.document.export_to_markdown())
            else:
                logger.warning("Chunk %d/%d failed, skipping.", idx, total)
        finally:
            _cleanup_result(result)

    if not markdown_parts:
        raise RuntimeError("All page chunks failed or deadline exceeded.")
    return "\n\n".join(markdown_parts)


def _download_models() -> None:
    from docling.utils.model_downloader import download_models

    download_models(
        progress=True,
        with_layout=True,
        with_tableformer=True,
        with_code_formula=True,
        with_picture_classifier=False,
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
    return sum(
        1 for j in _jobs.values() if j.status in (JobStatus.PENDING, JobStatus.RUNNING)
    )


@asynccontextmanager
async def lifespan(app: FastAPI):
    global _semaphore, _conversion_executor, _build_executor
    _semaphore = asyncio.Semaphore(_MAX_CONCURRENT)
    _conversion_executor = ThreadPoolExecutor(
        max_workers=_MAX_CONCURRENT, thread_name_prefix="docling-convert"
    )
    _build_executor = ThreadPoolExecutor(
        max_workers=1, thread_name_prefix="docling-build"
    )
    asyncio.create_task(_init())
    asyncio.create_task(_cleanup_loop())
    try:
        yield
    finally:
        _conversion_executor.shutdown(wait=False, cancel_futures=True)
        _build_executor.shutdown(wait=False, cancel_futures=True)


async def _init() -> None:
    global _ready
    try:
        logger.info("Downloading models ...")
        loop = asyncio.get_running_loop()
        await loop.run_in_executor(_build_executor, _download_models)

        for preset_name in QUALITY_PRESETS:
            try:
                converter = await loop.run_in_executor(
                    _build_executor, _build_converter, preset_name
                )
                _converters[preset_name] = PresetConverter(
                    preset=preset_name, converter=converter
                )
                logger.info("Converter [%s] ready.", preset_name)
            except Exception:
                _failed_presets.add(preset_name)
                logger.exception("Failed to build converter [%s].", preset_name)

        if not _converters:
            logger.error("No presets initialised; service remains unavailable.")
            return

        os.environ["HF_HUB_OFFLINE"] = "1"
        _ready = True
        logger.info(
            "Ready — %d/%d preset(s), max %d concurrent conversion(s).",
            len(_converters),
            len(QUALITY_PRESETS),
            _MAX_CONCURRENT,
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
    await _semaphore.acquire()
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
        pc = _converters[job.preset]

        # Whole-job timeout is enforced cooperatively: docling's
        # `document_timeout` stops feeding pages per chunk, and
        # `_convert_document` stops iterating chunks once the deadline
        # passes. No zombie threads, no process kill.
        async with pc.lock:
            try:
                markdown = await loop.run_in_executor(
                    _conversion_executor,
                    _convert_document,
                    pc.converter,
                    data,
                    filename,
                )
            except Exception as exc:
                elapsed = time.monotonic() - t0
                job.status = JobStatus.FAILED
                msg = str(exc).lower()
                if "not supported" in msg or "cannot convert" in msg:
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
                pc.job_count += 1

        elapsed = time.monotonic() - t0
        job.markdown = markdown
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
        del data
        gc.collect()
        _semaphore.release()
        # Recycle outside the per-job critical path so the rebuild doesn't
        # block this job's response nor stall other work on the event loop.
        pc = _converters.get(job.preset)
        if pc is not None and pc.job_count >= _CONVERTER_RECYCLE_AFTER > 0:
            async with pc.lock:
                try:
                    await pc.maybe_recycle()
                except Exception:
                    logger.exception("Converter recycle failed for [%s].", job.preset)


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
    return {
        "presets": list(QUALITY_PRESETS.keys()),
        "available": sorted(_converters.keys()),
        "unavailable": sorted(_failed_presets),
    }


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
    if preset not in _converters:
        raise HTTPException(
            status_code=503,
            detail=f"Preset '{preset}' failed to initialise and is unavailable.",
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
    """Poll a conversion job. Returns status and, when complete, the Markdown result.

    Does NOT delete on read: the cleanup loop is the sole owner of deletion,
    so a dropped connection after a completed response doesn't lose the result.
    """
    job = _jobs.get(job_id)
    if job is None:
        raise HTTPException(status_code=404, detail=f"Unknown job ID: {job_id!r}")
    if job.status == JobStatus.COMPLETED:
        return {"status": job.status, "markdown": job.markdown}
    if job.status == JobStatus.FAILED:
        return {"status": job.status, "detail": job.detail}
    return {"status": job.status}  # pending or running


if __name__ == "__main__":
    port_str = os.getenv("PORT")
    if not port_str:
        logger.error("PORT environment variable is required but not set")
        raise SystemExit(1)
    uvicorn.run("app:app", host="0.0.0.0", port=int(port_str), log_level="info")
