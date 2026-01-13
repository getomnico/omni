"""PDF text extraction module using pypdf"""

import io
import logging
import base64
from typing import Optional
import pypdf
from pydantic import BaseModel, field_validator

logger = logging.getLogger(__name__)


class PDFExtractionRequest(BaseModel):
    pdf_bytes: str  # Base64-encoded PDF bytes

    @field_validator("pdf_bytes")
    @classmethod
    def validate_and_decode_pdf_bytes(cls, v):
        """Validate and decode base64-encoded PDF bytes"""
        try:
            decoded = base64.b64decode(v)
            return decoded
        except Exception as e:
            raise ValueError(f"Invalid base64 PDF data: {e}")


class PDFExtractionResponse(BaseModel):
    text: str
    page_count: int
    error: Optional[str] = None


def extract_text_from_pdf(pdf_bytes: bytes) -> PDFExtractionResponse:
    """
    Extract text from PDF bytes using pypdf.

    Args:
        pdf_bytes: Raw PDF file bytes

    Returns:
        PDFExtractionResponse with extracted text, page count, and any errors
    """
    try:
        # Create a BytesIO object from the PDF bytes
        pdf_file = io.BytesIO(pdf_bytes)

        # Create PDF reader
        pdf_reader = pypdf.PdfReader(pdf_file)

        # Check if PDF is encrypted
        if pdf_reader.is_encrypted:
            logger.warning(
                "PDF is encrypted, attempting to decrypt with empty password"
            )
            try:
                pdf_reader.decrypt("")
            except Exception as e:
                return PDFExtractionResponse(
                    text="",
                    page_count=0,
                    error=f"Failed to decrypt password-protected PDF: {str(e)}",
                )

        # Extract text from all pages
        full_text = []
        page_count = len(pdf_reader.pages)

        for page_num, page in enumerate(pdf_reader.pages, 1):
            try:
                page_text = page.extract_text()
                if page_text:
                    full_text.append(page_text)
                    logger.debug(
                        f"Extracted {len(page_text)} characters from page {page_num}/{page_count}"
                    )
            except Exception as e:
                logger.warning(f"Failed to extract text from page {page_num}: {str(e)}")
                continue

        # Join all page texts with newlines
        extracted_text = "\n".join(full_text)

        if not extracted_text.strip():
            logger.warning(
                "No text content extracted from PDF - might be a scanned document"
            )
            return PDFExtractionResponse(
                text="",
                page_count=page_count,
                error="No text content found - PDF might contain only images or scanned pages",
            )

        logger.info(
            f"Successfully extracted {len(extracted_text)} characters from {page_count} pages"
        )
        return PDFExtractionResponse(
            text=extracted_text, page_count=page_count, error=None
        )

    except pypdf.errors.PdfReadError as e:
        logger.error(f"Invalid or corrupted PDF file: {str(e)}")
        return PDFExtractionResponse(
            text="", page_count=0, error=f"Invalid or corrupted PDF file: {str(e)}"
        )
    except Exception as e:
        logger.error(f"Unexpected error extracting PDF text: {str(e)}")
        return PDFExtractionResponse(
            text="", page_count=0, error=f"Unexpected error: {str(e)}"
        )
