use anyhow::{anyhow, Context, Result};
use calamine::{open_workbook_auto_from_rs, Reader};
use docx_rs::read_docx;
use quick_xml::events::Event;
use quick_xml::Reader as XmlReader;
use std::io::Cursor;
use tracing::{debug, warn};
use zip::ZipArchive;

/// Extract human-readable text content from raw file bytes based on MIME type.
///
/// When mime_type is `application/octet-stream`, falls back to extension-based
/// detection using the optional filename.
pub fn extract_content(data: &[u8], mime_type: &str, filename: Option<&str>) -> Result<String> {
    let effective_mime = if mime_type == "application/octet-stream" {
        filename
            .and_then(mime_from_extension)
            .unwrap_or_else(|| mime_type.to_string())
    } else {
        mime_type.to_string()
    };

    match effective_mime.as_str() {
        // Plain text formats — pass through as-is
        "text/plain" | "text/markdown" | "text/csv" => String::from_utf8(data.to_vec())
            .or_else(|_| Ok(String::from_utf8_lossy(data).into_owned())),

        "text/html" => {
            let body = String::from_utf8(data.to_vec())
                .or_else(|_| Ok::<_, anyhow::Error>(String::from_utf8_lossy(data).into_owned()))?;
            Ok(html_to_markdown(&body))
        }

        // PDF
        "application/pdf" => extract_pdf_text(data),

        // Modern Office formats
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            extract_docx_text(data)
        }
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            extract_excel_text(data)
        }
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            extract_pptx_text(data)
        }

        // Legacy Excel — calamine supports this natively
        "application/vnd.ms-excel" => extract_excel_text(data).or_else(|e| {
            warn!("Failed to extract text from legacy .xls file: {}", e);
            Ok(String::new())
        }),

        // Legacy Word — cannot be parsed with docx_rs (different binary format)
        "application/msword" => {
            debug!("Legacy .doc format is not supported, skipping");
            Ok(String::new())
        }

        // Legacy PowerPoint — binary format, not supported
        "application/vnd.ms-powerpoint" => {
            debug!("Legacy .ppt format is not supported, skipping");
            Ok(String::new())
        }

        _ => {
            debug!("Unsupported MIME type for extraction: '{}'", effective_mime);
            Ok(String::new())
        }
    }
}

/// Infer MIME type from a filename extension.
fn mime_from_extension(filename: &str) -> Option<String> {
    let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "xls" => "application/vnd.ms-excel",
        "doc" => "application/msword",
        "ppt" => "application/vnd.ms-powerpoint",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "md" | "markdown" => "text/markdown",
        _ => return None,
    };
    Some(mime.to_string())
}

fn html_to_markdown(html: &str) -> String {
    html2md::parse_html(html)
}

fn extract_pdf_text(data: &[u8]) -> Result<String> {
    let data_owned = data.to_vec();
    let result = std::panic::catch_unwind(move || {
        let mut doc = pdf_oxide::PdfDocument::from_bytes(data_owned)?;
        doc.extract_all_text()
    });

    match result {
        Ok(Ok(text)) => Ok(text.trim().to_string()),
        Ok(Err(e)) => Err(anyhow!("Failed to extract text from PDF: {}", e)),
        Err(_) => {
            warn!("PDF extraction panicked — likely a malformed PDF");
            Err(anyhow!("PDF extraction panicked due to malformed content"))
        }
    }
}

fn extract_docx_text(data: &[u8]) -> Result<String> {
    let docx = read_docx(data).context("Failed to read DOCX")?;
    let mut text = String::new();

    for child in &docx.document.children {
        match child {
            docx_rs::DocumentChild::Paragraph(paragraph) => {
                extract_paragraph_text(paragraph, &mut text);
                text.push('\n');
            }
            docx_rs::DocumentChild::Table(table) => {
                extract_table_text(table, &mut text);
            }
            _ => {}
        }
    }

    Ok(text.trim().to_string())
}

fn extract_paragraph_text(paragraph: &docx_rs::Paragraph, text: &mut String) {
    for para_child in &paragraph.children {
        if let docx_rs::ParagraphChild::Run(run) = para_child {
            for run_child in &run.children {
                if let docx_rs::RunChild::Text(t) = run_child {
                    text.push_str(&t.text);
                }
            }
        }
    }
}

fn extract_table_text(table: &docx_rs::Table, text: &mut String) {
    for row in &table.rows {
        let docx_rs::TableChild::TableRow(row) = row;
        let mut cells: Vec<String> = Vec::new();
        for cell in &row.cells {
            let docx_rs::TableRowChild::TableCell(cell) = cell;
            let mut cell_text = String::new();
            for child in &cell.children {
                if let docx_rs::TableCellContent::Paragraph(p) = child {
                    extract_paragraph_text(p, &mut cell_text);
                }
            }
            cells.push(cell_text);
        }
        text.push_str(&cells.join("\t"));
        text.push('\n');
    }
    text.push('\n');
}

fn extract_excel_text(data: &[u8]) -> Result<String> {
    let cursor = Cursor::new(data);
    let mut workbook =
        open_workbook_auto_from_rs(cursor).context("Failed to open Excel workbook")?;

    let mut text = String::new();
    let sheet_names = workbook.sheet_names().to_owned();

    for sheet_name in &sheet_names {
        text.push_str(&format!("Sheet: {}\n", sheet_name));
        if let Ok(range) = workbook.worksheet_range(sheet_name) {
            for row in range.rows() {
                let row_text: Vec<String> = row
                    .iter()
                    .map(|cell: &calamine::Data| cell.to_string())
                    .collect();
                text.push_str(&row_text.join("\t"));
                text.push('\n');
            }
        }
        text.push('\n');
    }

    Ok(text.trim().to_string())
}

fn extract_pptx_text(data: &[u8]) -> Result<String> {
    let cursor = Cursor::new(data);
    let mut archive = ZipArchive::new(cursor).context("Failed to read PPTX as ZIP")?;

    // Collect and sort slide names by numeric suffix for correct order
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
        let num_a = a
            .trim_start_matches("ppt/slides/slide")
            .trim_end_matches(".xml")
            .parse::<u32>()
            .unwrap_or(0);
        let num_b = b
            .trim_start_matches("ppt/slides/slide")
            .trim_end_matches(".xml")
            .parse::<u32>()
            .unwrap_or(0);
        num_a.cmp(&num_b)
    });

    let mut text = String::new();
    let mut slide_counter = 0;

    for name in slide_names {
        slide_counter += 1;
        text.push_str(&format!("Slide {}\n", slide_counter));
        let mut file = archive
            .by_name(&name)
            .context("Failed to read slide from PPTX")?;
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut file, &mut xml).context("Failed to read slide XML")?;
        text.push_str(&extract_text_from_pptx_xml(&xml)?);
        text.push_str("\n\n");
    }

    Ok(text.trim().to_string())
}

fn extract_text_from_pptx_xml(xml_content: &str) -> Result<String> {
    let mut reader = XmlReader::from_str(xml_content);
    reader.config_mut().trim_text(true);
    let mut text = String::new();
    let mut buf = Vec::new();
    let mut inside_text = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"a:t" => inside_text = true,
            Ok(Event::Text(e)) if inside_text => {
                let content = String::from_utf8_lossy(e.as_ref());
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
    use std::io::Write;

    #[test]
    fn test_extract_plain_text() {
        let data = b"Hello, world!";
        let result = extract_content(data, "text/plain", None).unwrap();
        assert_eq!(result, "Hello, world!");
    }

    #[test]
    fn test_extract_markdown() {
        let data = b"# Title\n\nSome content";
        let result = extract_content(data, "text/markdown", None).unwrap();
        assert_eq!(result, "# Title\n\nSome content");
    }

    #[test]
    fn test_extract_csv() {
        let data = b"name,age\nAlice,30\nBob,25";
        let result = extract_content(data, "text/csv", None).unwrap();
        assert!(result.contains("Alice"));
        assert!(result.contains("Bob"));
    }

    #[test]
    fn test_extract_html() {
        let data = b"<html><body><h1>Title</h1><p>Hello world</p></body></html>";
        let result = extract_content(data, "text/html", None).unwrap();
        assert!(result.contains("Title"));
        assert!(result.contains("Hello world"));
    }

    #[test]
    fn test_extract_docx() {
        let docx = docx_rs::Docx::new().add_paragraph(
            docx_rs::Paragraph::new().add_run(docx_rs::Run::new().add_text("Hello from DOCX")),
        );
        let mut buf = Vec::new();
        docx.build().pack(std::io::Cursor::new(&mut buf)).unwrap();

        let result = extract_content(
            &buf,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            None,
        )
        .unwrap();
        assert!(
            result.contains("Hello from DOCX"),
            "Expected 'Hello from DOCX', got: '{}'",
            result
        );
    }

    #[test]
    fn test_extract_docx_with_table() {
        let table = docx_rs::Table::new(vec![
            docx_rs::TableRow::new(vec![
                docx_rs::TableCell::new().add_paragraph(
                    docx_rs::Paragraph::new().add_run(docx_rs::Run::new().add_text("Name")),
                ),
                docx_rs::TableCell::new().add_paragraph(
                    docx_rs::Paragraph::new().add_run(docx_rs::Run::new().add_text("Age")),
                ),
            ]),
            docx_rs::TableRow::new(vec![
                docx_rs::TableCell::new().add_paragraph(
                    docx_rs::Paragraph::new().add_run(docx_rs::Run::new().add_text("Alice")),
                ),
                docx_rs::TableCell::new().add_paragraph(
                    docx_rs::Paragraph::new().add_run(docx_rs::Run::new().add_text("30")),
                ),
            ]),
        ]);

        let docx = docx_rs::Docx::new()
            .add_paragraph(
                docx_rs::Paragraph::new().add_run(docx_rs::Run::new().add_text("Before table")),
            )
            .add_table(table)
            .add_paragraph(
                docx_rs::Paragraph::new().add_run(docx_rs::Run::new().add_text("After table")),
            );

        let mut buf = Vec::new();
        docx.build().pack(std::io::Cursor::new(&mut buf)).unwrap();

        let result = extract_content(
            &buf,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            None,
        )
        .unwrap();

        assert!(
            result.contains("Before table"),
            "Missing paragraph before table"
        );
        assert!(result.contains("Name\tAge"), "Missing table header row");
        assert!(result.contains("Alice\t30"), "Missing table data row");
        assert!(
            result.contains("After table"),
            "Missing paragraph after table"
        );
    }

    #[test]
    fn test_extract_xlsx() {
        let data = create_test_xlsx(&[&["Name", "Age"], &["Alice", "30"]]);
        let result = extract_content(
            &data,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            None,
        )
        .unwrap();
        assert!(result.contains("Name") && result.contains("Alice"));
    }

    #[test]
    fn test_extract_pptx() {
        let data = create_test_pptx(&["Welcome slide", "Second slide"]);
        let result = extract_content(
            &data,
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            None,
        )
        .unwrap();
        assert!(result.contains("Welcome slide"));
        assert!(result.contains("Second slide"));
    }

    #[test]
    fn test_unsupported_mime_returns_empty() {
        let result = extract_content(&[0x89, 0x50, 0x4E, 0x47], "image/png", None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_legacy_doc_returns_empty() {
        let result = extract_content(b"fake doc data", "application/msword", None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_octet_stream_with_extension_fallback() {
        let data = b"Hello, world!";
        let result = extract_content(data, "application/octet-stream", Some("notes.txt")).unwrap();
        assert_eq!(result, "Hello, world!");
    }

    #[test]
    fn test_mime_from_extension() {
        assert_eq!(
            mime_from_extension("report.pdf").unwrap(),
            "application/pdf"
        );
        assert_eq!(
            mime_from_extension("data.xlsx").unwrap(),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );
        assert!(mime_from_extension("image.png").is_none());
    }

    // ── Test helpers ──

    fn create_test_xlsx(rows: &[&[&str]]) -> Vec<u8> {
        use zip::write::SimpleFileOptions as FileOptions;

        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);

            zip.start_file("[Content_Types].xml", FileOptions::default())
                .unwrap();
            write!(zip, r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"#).unwrap();

            zip.start_file("_rels/.rels", FileOptions::default())
                .unwrap();
            write!(zip, r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#).unwrap();

            zip.start_file("xl/_rels/workbook.xml.rels", FileOptions::default())
                .unwrap();
            write!(zip, r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
</Relationships>"#).unwrap();

            zip.start_file("xl/workbook.xml", FileOptions::default())
                .unwrap();
            write!(
                zip,
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
          xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="Sheet1" sheetId="1" r:id="rId1"/>
  </sheets>
</workbook>"#
            )
            .unwrap();

            let mut sheet_xml = String::from(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData>"#,
            );
            for (row_idx, row) in rows.iter().enumerate() {
                sheet_xml.push_str(&format!("\n    <row r=\"{}\">", row_idx + 1));
                for (col_idx, cell_val) in row.iter().enumerate() {
                    let col_letter = (b'A' + col_idx as u8) as char;
                    sheet_xml.push_str(&format!(
                        "<c r=\"{}{}\" t=\"inlineStr\"><is><t>{}</t></is></c>",
                        col_letter,
                        row_idx + 1,
                        cell_val
                    ));
                }
                sheet_xml.push_str("</row>");
            }
            sheet_xml.push_str("\n  </sheetData>\n</worksheet>");

            zip.start_file("xl/worksheets/sheet1.xml", FileOptions::default())
                .unwrap();
            write!(zip, "{}", sheet_xml).unwrap();
            zip.finish().unwrap();
        }
        buf
    }

    fn create_test_pptx(slide_texts: &[&str]) -> Vec<u8> {
        use zip::write::SimpleFileOptions as FileOptions;

        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);

            for (i, text) in slide_texts.iter().enumerate() {
                let slide_name = format!("ppt/slides/slide{}.xml", i + 1);
                zip.start_file(&slide_name, FileOptions::default()).unwrap();
                write!(
                    zip,
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld><p:spTree><p:sp><p:txBody>
    <a:p><a:r><a:t>{}</a:t></a:r></a:p>
  </p:txBody></p:sp></p:spTree></p:cSld>
</p:sld>"#,
                    text
                )
                .unwrap();
            }

            zip.finish().unwrap();
        }
        buf
    }
}
