use anyhow::{Context, Result, anyhow};
use quick_xml::Reader as XmlReader;
use quick_xml::events::Event;
use std::io::{BufReader, Cursor};
use zip::ZipArchive;

use super::is_textual_spreadsheet_cell;

pub(super) fn extract_xlsx_text_filtered(data: &[u8], max_rows: usize) -> Result<String> {
    extract_xlsx_text_streaming(data, max_rows)
}

const MAX_XLSX_SHARED_STRINGS_TEXT_BYTES: usize = 32 * 1024 * 1024;
const MAX_XLSX_SHARED_STRING_COUNT: usize = 1_000_000;

fn extract_xlsx_text_streaming(data: &[u8], max_rows: usize) -> Result<String> {
    let cursor = Cursor::new(data);
    let mut archive = ZipArchive::new(cursor).context("Failed to read XLSX as ZIP")?;

    let shared_strings = read_xlsx_shared_strings(&mut archive)?;
    let sheets = read_xlsx_sheet_entries(&mut archive)?;

    let mut text = String::new();
    let mut rows_written = 0usize;

    for (sheet_name, sheet_path) in sheets {
        if rows_written >= max_rows {
            break;
        }

        let sheet_text = read_xlsx_sheet_rows(
            &mut archive,
            &sheet_path,
            &shared_strings,
            max_rows,
            &mut rows_written,
        )?;

        if !sheet_text.is_empty() {
            text.push_str(&format!("Sheet: {}\n", sheet_name));
            text.push_str(&sheet_text);
            text.push('\n');
        }
    }

    Ok(text.trim().to_string())
}

fn read_xlsx_sheet_entries(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
) -> Result<Vec<(String, String)>> {
    let workbook = read_zip_entry_to_string(archive, "xl/workbook.xml")?;
    let rels = read_zip_entry_to_string(archive, "xl/_rels/workbook.xml.rels")?;
    let sheet_rels = parse_xlsx_workbook_relationships(&rels)?;
    let workbook_sheets = parse_xlsx_workbook_sheets(&workbook)?;

    let mut sheets = Vec::new();
    for (name, rel_id) in workbook_sheets {
        if let Some(path) = sheet_rels.get(&rel_id) {
            sheets.push((name, path.clone()));
        }
    }

    if sheets.is_empty() {
        let mut worksheet_paths: Vec<String> = (0..archive.len())
            .filter_map(|i| {
                let name = archive.by_index(i).ok()?.name().to_string();
                if name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml") {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();
        worksheet_paths.sort_by_key(|name| worksheet_sort_key(name));
        sheets = worksheet_paths
            .into_iter()
            .enumerate()
            .map(|(idx, path)| (format!("Sheet {}", idx + 1), path))
            .collect();
    }

    Ok(sheets)
}

fn read_zip_entry_to_string(archive: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<String> {
    let mut file = archive
        .by_name(name)
        .with_context(|| format!("Failed to read {} from XLSX", name))?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut file, &mut text)
        .with_context(|| format!("Failed to read {} as UTF-8", name))?;
    Ok(text)
}

fn parse_xlsx_workbook_sheets(workbook_xml: &str) -> Result<Vec<(String, String)>> {
    let mut reader = XmlReader::from_str(workbook_xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut sheets = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) if e.name().as_ref() == b"sheet" => {
                let name = xml_attr_value(&e, b"name").unwrap_or_else(|| "Sheet".to_string());
                if let Some(rel_id) = xml_attr_value(&e, b"r:id") {
                    sheets.push((name, rel_id));
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("Error reading XLSX workbook XML: {}", e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(sheets)
}

fn parse_xlsx_workbook_relationships(
    rels_xml: &str,
) -> Result<std::collections::HashMap<String, String>> {
    let mut reader = XmlReader::from_str(rels_xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut relationships = std::collections::HashMap::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) if e.name().as_ref() == b"Relationship" => {
                if let (Some(id), Some(target)) =
                    (xml_attr_value(&e, b"Id"), xml_attr_value(&e, b"Target"))
                {
                    relationships.insert(id, normalize_xlsx_relationship_target(&target));
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("Error reading XLSX workbook relationships: {}", e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(relationships)
}

fn normalize_xlsx_relationship_target(target: &str) -> String {
    let normalized = target.trim_start_matches('/');
    if normalized.starts_with("xl/") {
        normalized.to_string()
    } else {
        format!("xl/{}", normalized)
    }
}

fn xml_attr_value(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|attr| attr.key.as_ref() == key)
        .map(|attr| String::from_utf8_lossy(attr.value.as_ref()).into_owned())
}

fn worksheet_sort_key(name: &str) -> u32 {
    name.trim_start_matches("xl/worksheets/sheet")
        .trim_end_matches(".xml")
        .parse::<u32>()
        .unwrap_or(0)
}

fn read_xlsx_shared_strings(archive: &mut ZipArchive<Cursor<&[u8]>>) -> Result<Vec<String>> {
    let Ok(file) = archive.by_name("xl/sharedStrings.xml") else {
        return Ok(Vec::new());
    };

    let mut reader = XmlReader::from_reader(BufReader::new(file));
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut strings = Vec::new();
    let mut current = String::new();
    let mut inside_si = false;
    let mut inside_text = false;
    let mut total_text_bytes = 0usize;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name().as_ref() == b"si" => {
                inside_si = true;
                current.clear();
            }
            Ok(Event::Start(e)) if inside_si && e.name().as_ref() == b"t" => {
                inside_text = true;
            }
            Ok(Event::Text(e)) if inside_text => {
                current.push_str(&String::from_utf8_lossy(e.as_ref()));
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"t" => {
                inside_text = false;
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"si" => {
                total_text_bytes = total_text_bytes.saturating_add(current.len());
                if total_text_bytes > MAX_XLSX_SHARED_STRINGS_TEXT_BYTES {
                    return Err(anyhow!(
                        "XLSX shared strings exceed {} byte safety limit",
                        MAX_XLSX_SHARED_STRINGS_TEXT_BYTES
                    ));
                }
                if strings.len() >= MAX_XLSX_SHARED_STRING_COUNT {
                    return Err(anyhow!(
                        "XLSX shared strings exceed {} item safety limit",
                        MAX_XLSX_SHARED_STRING_COUNT
                    ));
                }
                strings.push(current.clone());
                inside_si = false;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("Error reading XLSX shared strings: {}", e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(strings)
}

fn read_xlsx_sheet_rows(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    sheet_path: &str,
    shared_strings: &[String],
    max_rows: usize,
    rows_written: &mut usize,
) -> Result<String> {
    let file = archive
        .by_name(sheet_path)
        .with_context(|| format!("Failed to read worksheet {} from XLSX", sheet_path))?;
    let mut reader = XmlReader::from_reader(BufReader::new(file));
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut text = String::new();

    let mut inside_row = false;
    let mut inside_value = false;
    let mut inside_inline_text = false;
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell_type: Option<String> = None;
    let mut current_cell_value = String::new();
    let mut current_inline_text = String::new();

    loop {
        if *rows_written >= max_rows {
            break;
        }

        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name().as_ref() == b"row" => {
                inside_row = true;
                current_row.clear();
            }
            Ok(Event::Start(e)) if inside_row && e.name().as_ref() == b"c" => {
                current_cell_type = xml_attr_value(&e, b"t");
                current_cell_value.clear();
                current_inline_text.clear();
            }
            Ok(Event::Start(e)) if inside_row && e.name().as_ref() == b"v" => {
                inside_value = true;
            }
            Ok(Event::Start(e)) if inside_row && e.name().as_ref() == b"t" => {
                inside_inline_text = true;
            }
            Ok(Event::Text(e)) if inside_value => {
                current_cell_value.push_str(&String::from_utf8_lossy(e.as_ref()));
            }
            Ok(Event::Text(e)) if inside_inline_text => {
                current_inline_text.push_str(&String::from_utf8_lossy(e.as_ref()));
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"v" => {
                inside_value = false;
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"t" => {
                inside_inline_text = false;
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"c" => {
                if let Some(cell_text) = resolve_xlsx_cell_text(
                    current_cell_type.as_deref(),
                    &current_cell_value,
                    &current_inline_text,
                    shared_strings,
                ) {
                    let trimmed = cell_text.trim();
                    if is_textual_spreadsheet_cell(trimmed) {
                        current_row.push(trimmed.to_string());
                    }
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"row" => {
                if !current_row.is_empty() {
                    text.push_str(&current_row.join("\t"));
                    text.push('\n');
                    *rows_written += 1;
                }
                inside_row = false;
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(anyhow!(
                    "Error reading XLSX worksheet {}: {}",
                    sheet_path,
                    e
                ));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(text)
}

fn resolve_xlsx_cell_text(
    cell_type: Option<&str>,
    cell_value: &str,
    inline_text: &str,
    shared_strings: &[String],
) -> Option<String> {
    match cell_type {
        Some("s") => cell_value
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|idx| shared_strings.get(idx).cloned()),
        Some("inlineStr") => Some(inline_text.to_string()),
        Some("str") => Some(cell_value.to_string()),
        _ => Some(cell_value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_extractor::extract_content;
    use std::io::Write;

    #[test]
    fn test_extract_xlsx_filters_numeric_only_cells() {
        let data = create_test_xlsx(&[
            &["Name", "Age", "Cost"],
            &["Alice", "30", "$10.00"],
            &["123", "456", "2024-01-31"],
            &["Q4 revenue", "1.2e6", "12%"],
            &["東京", "99", "---"],
        ]);
        let result = extract_content(
            &data,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            None,
        )
        .unwrap();

        assert!(result.contains("Sheet: Sheet1"));
        assert!(result.contains("Name\tAge\tCost"));
        assert!(result.contains("Alice"));
        assert!(result.contains("Q4 revenue"));
        assert!(result.contains("東京"));
        assert!(!result.contains("30"));
        assert!(!result.contains("$10.00"));
        assert!(!result.contains("123\t456"));
        assert!(!result.contains("1.2e6"));
        assert!(!result.contains("2024-01-31"));
    }

    #[test]
    fn test_extract_xlsx_resolves_shared_strings() {
        let data = create_test_xlsx_with_shared_strings(&[
            &["Name", "Age", "Cost"],
            &["Alice", "30", "$10.00"],
        ]);
        let result = extract_content(
            &data,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            None,
        )
        .unwrap();

        assert!(result.contains("Sheet: Sheet1"));
        assert!(result.contains("Name\tAge\tCost"));
        assert!(result.contains("Alice"));
        assert!(!result.contains("30"));
        assert!(!result.contains("$10.00"));
    }

    #[test]
    fn test_extract_xlsx_streaming_applies_row_limit() {
        let data = create_test_xlsx(&[&["Header"], &["Alice"], &["Bob"], &["Carol"]]);
        let result = extract_xlsx_text_streaming(&data, 2).unwrap();

        assert!(result.contains("Header"));
        assert!(result.contains("Alice"));
        assert!(!result.contains("Bob"));
        assert!(!result.contains("Carol"));
    }

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

    fn create_test_xlsx_with_shared_strings(rows: &[&[&str]]) -> Vec<u8> {
        use zip::write::SimpleFileOptions as FileOptions;

        let mut shared_strings = Vec::new();
        let mut sheet_xml = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData>"#,
        );
        for (row_idx, row) in rows.iter().enumerate() {
            sheet_xml.push_str(&format!("\n    <row r=\"{}\">", row_idx + 1));
            for (col_idx, cell_val) in row.iter().enumerate() {
                let col_letter = (b'A' + col_idx as u8) as char;
                let shared_idx = shared_strings.len();
                shared_strings.push(*cell_val);
                sheet_xml.push_str(&format!(
                    "<c r=\"{}{}\" t=\"s\"><v>{}</v></c>",
                    col_letter,
                    row_idx + 1,
                    shared_idx
                ));
            }
            sheet_xml.push_str("</row>");
        }
        sheet_xml.push_str("\n  </sheetData>\n</worksheet>");

        let mut shared_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{}" uniqueCount="{}">"#,
            shared_strings.len(),
            shared_strings.len()
        );
        for value in &shared_strings {
            shared_xml.push_str(&format!("<si><t>{}</t></si>", value));
        }
        shared_xml.push_str("</sst>");

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
  <Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>
</Types>"#).unwrap();

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
  <sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#
            )
            .unwrap();

            zip.start_file("xl/sharedStrings.xml", FileOptions::default())
                .unwrap();
            write!(zip, "{}", shared_xml).unwrap();

            zip.start_file("xl/worksheets/sheet1.xml", FileOptions::default())
                .unwrap();
            write!(zip, "{}", sheet_xml).unwrap();
            zip.finish().unwrap();
        }
        buf
    }
}
