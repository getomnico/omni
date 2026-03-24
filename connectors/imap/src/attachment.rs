use anyhow::{Context, Result};
use mailparse::ParsedMail;
use tracing::{debug, warn};

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
                    debug!("Extracted {} chars from attachment '{}'", text.len(), name);
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
    let data = mail
        .get_body_raw()
        .context("Failed to get attachment bytes")?;
    shared::content_extractor::extract_text(&data, mime, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_supported_known_types() {
        assert!(is_supported("application/pdf"));
        assert!(is_supported(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        ));
        assert!(is_supported(
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        ));
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
}
