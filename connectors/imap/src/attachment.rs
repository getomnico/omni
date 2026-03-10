use anyhow::{anyhow, Context, Result};
use calamine::{open_workbook_auto_from_rs, Reader};
use docx_rs::read_docx;
use mailparse::ParsedMail;
use quick_xml::events::Event;
use quick_xml::Reader as XmlReader;
use std::io::Cursor;
use tracing::{debug, warn};
use zip::ZipArchive;

/// An extracted attachment with its filename, MIME type, and text content.
#[derive(Debug, Clone)]
pub struct ExtractedAttachment {
    pub filename: String,
    pub mime_type: String,
    pub text: String,
}

/// MIME types from which we can extract indexable text.
const SUPPORTED_MIME_TYPES: &[&str] = &[
    "application/pdf",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.ms-excel",
    "text/plain",
    "text/html",
    "text/csv",
    "text/markdown",
];

/// Check whether a MIME type is one we can extract text from.
fn is_supported(mime: &str) -> bool {
    SUPPORTED_MIME_TYPES.contains(&mime)
}

/// Recursively walk the MIME tree and extract text from all supported attachments.
pub fn extract_attachments(mail: &ParsedMail) -> Vec<ExtractedAttachment> {
    let mut out = Vec::new();
    collect_attachments(mail, &mut out);
    out
}

fn collect_attachments(mail: &ParsedMail, out: &mut Vec<ExtractedAttachment>) {
    let declared_ct = mail.ctype.mimetype.to_ascii_lowercase();

    // Resolve a filename from Content-Disposition or Content-Type parameters.
    // Per RFC 2183, a part may have `Content-Disposition: attachment` without
    // a filename parameter — in that case we generate a synthetic name from
    // the MIME type so the part is not silently dropped.
    let filename = attachment_filename(mail).or_else(|| {
        let disposition = mail.get_content_disposition();
        if disposition.disposition == mailparse::DispositionType::Attachment {
            Some(synthetic_filename(&declared_ct))
        } else {
            None
        }
    });

    if let Some(name) = filename {
        // Many mail clients send attachments as `application/octet-stream`
        // regardless of the actual file type.  Infer from the extension.
        let effective_ct = if declared_ct == "application/octet-stream" {
            mime_from_extension(&name).unwrap_or(declared_ct.clone())
        } else {
            declared_ct.clone()
        };

        if is_supported(&effective_ct) {
            match extract_text_from_part(mail, &effective_ct) {
                Ok(text) if !text.trim().is_empty() => {
                    debug!(
                        "Extracted {} chars from attachment '{}'",
                        text.len(),
                        name
                    );
                    out.push(ExtractedAttachment {
                        filename: name,
                        mime_type: effective_ct,
                        text,
                    });
                }
                Ok(_) => {
                    debug!("Attachment '{}' produced empty text, skipping", name);
                }
                Err(e) => {
                    warn!("Failed to extract text from attachment '{}': {}", name, e);
                }
            }
        }
    }

    for sub in &mail.subparts {
        collect_attachments(sub, out);
    }
}

/// Generate a fallback filename from a MIME type for attachments that lack a
/// `filename` parameter (valid per RFC 2183 §2.2).
fn synthetic_filename(mime: &str) -> String {
    let ext = match mime {
        "application/pdf" => "pdf",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "application/vnd.ms-excel" => "xls",
        "text/plain" => "txt",
        "text/html" => "html",
        "text/csv" => "csv",
        "text/markdown" => "md",
        _ => "bin",
    };
    format!("attachment.{}", ext)
}

/// Infer MIME type from a filename extension.  Only used when the declared
/// Content-Type is `application/octet-stream`.
fn mime_from_extension(filename: &str) -> Option<String> {
    let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "xls" => "application/vnd.ms-excel",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "md" | "markdown" => "text/markdown",
        _ => return None,
    };
    Some(mime.to_string())
}

/// Resolve the filename from Content-Disposition or Content-Type parameters.
fn attachment_filename(mail: &ParsedMail) -> Option<String> {
    let disposition = mail.get_content_disposition();
    // Prefer Content-Disposition filename.
    if let Some(name) = disposition.params.get("filename") {
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    // Fall back to Content-Type name parameter.
    if let Some(name) = mail.ctype.params.get("name") {
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

/// Extract plaintext from a single MIME part based on its content type.
fn extract_text_from_part(mail: &ParsedMail, mime: &str) -> Result<String> {
    match mime {
        "text/plain" | "text/csv" | "text/markdown" => {
            mail.get_body().context("Failed to decode text attachment body")
        }
        "text/html" => {
            let body = mail.get_body().context("Failed to decode HTML attachment body")?;
            Ok(html_to_text(&body))
        }
        "application/pdf" => {
            let data = mail.get_body_raw().context("Failed to get PDF attachment bytes")?;
            extract_pdf_text(&data)
        }
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            let data = mail.get_body_raw().context("Failed to get DOCX attachment bytes")?;
            extract_docx_text(&data)
        }
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/vnd.ms-excel" => {
            let data = mail.get_body_raw().context("Failed to get Excel attachment bytes")?;
            extract_excel_text(&data)
        }
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            let data = mail.get_body_raw().context("Failed to get PPTX attachment bytes")?;
            extract_pptx_text(&data)
        }
        _ => Ok(String::new()),
    }
}

/// Column width used when rendering HTML to plain text.
const HTML_TEXT_WIDTH: usize = 100;

fn html_to_text(html: &str) -> String {
    html2text::from_read(html.as_bytes(), HTML_TEXT_WIDTH)
}

// ── File format extractors ───────────────────────────────────────────────────
// Mirrors the implementations in connectors/filesystem/src/content_extractor.rs
// to keep behaviour consistent across connectors.

fn extract_pdf_text(data: &[u8]) -> Result<String> {
    // Wrap PDF extraction in catch_unwind because pdf_oxide can panic on
    // certain malformed PDFs or PDFs with multi-byte UTF-8 characters in
    // font handling code.
    let data_owned = data.to_vec();
    let result = std::panic::catch_unwind(move || {
        let mut doc = pdf_oxide::PdfDocument::from_bytes(data_owned)?;
        doc.extract_all_text()
    });

    match result {
        Ok(Ok(text)) => Ok(text.trim().to_string()),
        Ok(Err(e)) => Err(anyhow!("Failed to extract text from PDF: {}", e)),
        Err(_) => {
            warn!("PDF extraction panicked - skipping this attachment");
            Err(anyhow!("PDF extraction panicked due to malformed content"))
        }
    }
}

fn extract_docx_text(data: &[u8]) -> Result<String> {
    let docx = read_docx(data).context("Failed to read DOCX")?;
    let mut text = String::new();
    for child in &docx.document.children {
        if let docx_rs::DocumentChild::Paragraph(paragraph) = child {
            for para_child in &paragraph.children {
                if let docx_rs::ParagraphChild::Run(run) = para_child {
                    for run_child in &run.children {
                        if let docx_rs::RunChild::Text(t) = run_child {
                            text.push_str(&t.text);
                        }
                    }
                }
            }
            text.push('\n');
        }
    }
    Ok(text.trim().to_string())
}

fn extract_excel_text(data: &[u8]) -> Result<String> {
    let cursor = Cursor::new(data);
    let mut workbook =
        open_workbook_auto_from_rs(cursor).context("Failed to open Excel workbook")?;
    let mut text = String::new();
    let sheet_names = workbook.sheet_names().to_owned();
    for sheet_name in &sheet_names {
        text.push_str(&format!("Sheet: {}\n", sheet_name));
        if let Some(Ok(range)) = workbook.worksheet_range(sheet_name) {
            for row in range.rows() {
                let row_text: Vec<String> = row.iter().map(|cell| cell.to_string()).collect();
                text.push_str(&row_text.join("\t"));
                text.push('\n');
            }
        }
        text.push('\n');
    }
    Ok(text.trim().to_string())
}

fn extract_pptx_text(data: &[u8]) -> Result<String> {
    let cursor = Cursor::new(data.to_vec());
    let mut archive = ZipArchive::new(cursor).context("Failed to read PPTX as ZIP")?;
    let mut text = String::new();
    let mut slide_counter = 0;

    // Collect slide file names first (ZipArchive borrows prevent inline iteration).
    // Sort by the numeric suffix so slides are always in logical order,
    // regardless of ZIP directory iteration order (not guaranteed by spec).
    let mut slide_names: Vec<String> = (0..archive.len())
        .filter_map(|i| {
            let name = archive.by_index(i).ok()?.name().to_string();
            if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    slide_names.sort_by(|a, b| {
        let num_a = a.trim_start_matches("ppt/slides/slide").trim_end_matches(".xml");
        let num_b = b.trim_start_matches("ppt/slides/slide").trim_end_matches(".xml");
        let na = num_a.parse::<u32>().unwrap_or(0);
        let nb = num_b.parse::<u32>().unwrap_or(0);
        na.cmp(&nb)
    });

    for name in slide_names {
        slide_counter += 1;
        text.push_str(&format!("Slide {}\n", slide_counter));
        let mut file = archive.by_name(&name).context("Failed to read slide from PPTX")?;
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut file, &mut xml)
            .context("Failed to read slide XML")?;
        text.push_str(&extract_text_from_pptx_xml(&xml)?);
        text.push_str("\n\n");
    }
    Ok(text.trim().to_string())
}

fn extract_text_from_pptx_xml(xml_content: &str) -> Result<String> {
    let mut reader = XmlReader::from_str(xml_content);
    reader.trim_text(true);
    let mut text = String::new();
    let mut buf = Vec::new();
    let mut inside_text = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"a:t" => inside_text = true,
            Ok(Event::Text(e)) if inside_text => {
                let content = e.unescape().context("Failed to unescape XML text")?;
                text.push_str(&content);
                text.push(' ');
            }
            Ok(Event::End(ref e)) => match e.name().as_ref() {
                b"a:t" => inside_text = false,
                b"a:p" => text.push('\n'),
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("Error reading PPTX XML: {}", e)),
            _ => {}
        }
        buf.clear();
    }
    Ok(text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_supported_known_types() {
        assert!(is_supported("application/pdf"));
        assert!(is_supported("application/vnd.openxmlformats-officedocument.wordprocessingml.document"));
        assert!(is_supported("application/vnd.openxmlformats-officedocument.presentationml.presentation"));
        assert!(is_supported("application/vnd.ms-excel"));
        assert!(is_supported("text/plain"));
        assert!(is_supported("text/html"));
        assert!(is_supported("text/csv"));
    }

    #[test]
    fn test_is_supported_unknown_types() {
        assert!(!is_supported("image/png"));
        assert!(!is_supported("application/octet-stream"));
        assert!(!is_supported("application/msword"));
        assert!(!is_supported("application/vnd.ms-powerpoint"));
        assert!(!is_supported("video/mp4"));
    }

    #[test]
    fn test_extract_docx_text() {
        let docx = docx_rs::Docx::new().add_paragraph(
            docx_rs::Paragraph::new().add_run(docx_rs::Run::new().add_text("Hello from DOCX")),
        );
        let mut buf = Vec::new();
        docx.build().pack(std::io::Cursor::new(&mut buf)).unwrap();
        let result = super::extract_docx_text(&buf).unwrap();
        assert!(
            result.contains("Hello from DOCX"),
            "Expected 'Hello from DOCX', got: '{}'",
            result
        );
    }

    #[test]
    fn test_extract_pptx_text() {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            zip.start_file("ppt/slides/slide1.xml", zip::write::FileOptions::default())
                .unwrap();
            write!(
                zip,
                r#"<?xml version="1.0"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld><p:spTree><p:sp><p:txBody>
    <a:p><a:r><a:t>Slide text here</a:t></a:r></a:p>
  </p:txBody></p:sp></p:spTree></p:cSld>
</p:sld>"#
            )
            .unwrap();
            zip.finish().unwrap();
        }
        let result = super::extract_pptx_text(&buf).unwrap();
        assert!(
            result.contains("Slide text here"),
            "Expected slide text, got: '{}'",
            result
        );
    }
}
