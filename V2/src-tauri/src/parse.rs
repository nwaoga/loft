//! M4 — file parsers.
//!
//! Turns a file's bytes into plain-text sections (one per page / sheet / slide
//! where the format has them) ready for chunking. Supported: text / Markdown /
//! code / data files, PDF, Word (.docx), Excel (.xlsx/.xls/.ods), PowerPoint
//! (.pptx).

use std::io::{Cursor, Read};
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub text: String,
    /// 1-based page / sheet / slide number where the format has one.
    pub page: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Pdf,
    Docx,
    Pptx,
    Sheet,
    /// A picture; indexed by the text that can be read off it (OCR).
    Image,
}

/// Bump when parsing gets better at files that used to yield nothing (see
/// `Store::upgrade_parsers`). 2: scanned PDF pages and images are read by OCR.
pub const PARSER_VERSION: u32 = 2;

/// A PDF page with fewer letters and digits than this is taken for a scan.
const MIN_PAGE_TEXT: usize = 16;
/// An image must show at least this many letters and digits to be indexed.
const MIN_IMAGE_TEXT: usize = 12;
/// Most pages of one PDF that are read by OCR (each takes a fraction of a second).
pub const MAX_OCR_PAGES: usize = 150;

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff", "webp"];

/// How much real text there is: letters and digits, not layout or noise.
fn letters(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphanumeric()).count()
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Pdf => "pdf",
            Kind::Docx => "docx",
            Kind::Pptx => "pptx",
            Kind::Sheet => "sheet",
            Kind::Image => "image",
        }
    }

    /// Largest file we will read for this kind.
    pub fn max_bytes(self) -> u64 {
        match self {
            Kind::Text => 5 * 1024 * 1024,
            Kind::Image => 25 * 1024 * 1024,
            _ => 100 * 1024 * 1024,
        }
    }
}

const TEXT_EXTENSIONS: &[&str] = &[
    // prose + data
    "txt", "md", "markdown", "mdx", "rst", "adoc", "org", "log", "csv", "tsv", "json", "jsonl",
    "yaml", "yml", "toml", "xml", "html", "htm", "ini", "cfg", "conf", "env", "properties", "tex",
    // code
    "rs", "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "svelte", "vue", "java", "kt", "kts",
    "scala", "c", "h", "cpp", "cc", "hpp", "cs", "csproj", "sln", "razor", "cshtml", "go", "rb",
    "php", "swift", "m", "mm", "dart", "lua", "r", "jl", "sql", "sh", "bash", "zsh", "fish",
    "bat", "cmd", "ps1", "psm1", "css", "scss", "sass", "less", "graphql", "proto", "tf",
    "gradle", "cmake", "dockerfile", "makefile", "vb", "vbs", "bas", "cls", "frm", "vbp",
];

/// Which parser handles this path, if any (by extension).
pub fn kind_for(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => Some(Kind::Pdf),
        "docx" | "docm" => Some(Kind::Docx),
        "pptx" | "pptm" => Some(Kind::Pptx),
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => Some(Kind::Sheet),
        e if TEXT_EXTENSIONS.contains(&e) => Some(Kind::Text),
        e if IMAGE_EXTENSIONS.contains(&e) => Some(Kind::Image),
        _ => None,
    }
}

/// Extract cleaned text sections. An empty result means "nothing indexable"
/// (e.g. a binary file with a text extension, or a scanned PDF with no text).
pub fn parse(kind: Kind, bytes: &[u8]) -> Result<Vec<Section>, String> {
    let sections = match kind {
        Kind::Text => match decode_text(bytes) {
            Some(text) => vec![Section { text, page: None }],
            None => Vec::new(),
        },
        Kind::Pdf => parse_pdf(bytes)?,
        Kind::Docx => vec![Section {
            text: parse_docx(bytes)?,
            page: None,
        }],
        Kind::Pptx => parse_pptx(bytes)?,
        Kind::Sheet => parse_sheet(bytes)?,
        Kind::Image => parse_image(bytes),
    };
    Ok(sections
        .into_iter()
        .map(|s| Section {
            text: clean(&s.text),
            page: s.page,
        })
        .filter(|s| !s.text.is_empty())
        .collect())
}

/// Normalise newlines, strip NULs and trailing spaces, collapse blank runs.
fn clean(s: &str) -> String {
    let s = s.replace("\r\n", "\n").replace('\r', "\n").replace('\0', "");
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

// ------------------------------- text ------------------------------------

/// Decode text as UTF-8 (with or without BOM), UTF-16 (with BOM), or
/// Windows-1252 as a fallback. Returns None for binary content.
pub fn decode_text(bytes: &[u8]) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return decode_text(rest);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let le = bytes[0] == 0xFF;
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|b| {
                if le {
                    u16::from_le_bytes([b[0], b[1]])
                } else {
                    u16::from_be_bytes([b[0], b[1]])
                }
            })
            .collect();
        return Some(String::from_utf16_lossy(&units));
    }
    // NUL bytes in the head => binary.
    if bytes.iter().take(8000).any(|&b| b == 0) {
        return None;
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Some(s.to_string()),
        Err(_) => Some(bytes.iter().map(|&b| windows_1252(b)).collect()),
    }
}

/// Windows-1252 byte -> char (Latin-1 plus the 0x80–0x9F punctuation block).
fn windows_1252(b: u8) -> char {
    match b {
        0x80 => '€',
        0x82 => '‚',
        0x83 => 'ƒ',
        0x84 => '„',
        0x85 => '…',
        0x86 => '†',
        0x87 => '‡',
        0x88 => 'ˆ',
        0x89 => '‰',
        0x8A => 'Š',
        0x8B => '‹',
        0x8C => 'Œ',
        0x8E => 'Ž',
        0x91 => '‘',
        0x92 => '’',
        0x93 => '“',
        0x94 => '”',
        0x95 => '•',
        0x96 => '–',
        0x97 => '—',
        0x98 => '˜',
        0x99 => '™',
        0x9A => 'š',
        0x9B => '›',
        0x9C => 'œ',
        0x9E => 'ž',
        0x9F => 'Ÿ',
        _ => b as char,
    }
}

// -------------------------------- pdf ------------------------------------

/// The text on a picture, if it shows enough to be worth indexing. A picture
/// that cannot be read (no OCR on this system, an odd format, a photo of a
/// landscape) is simply "nothing indexable", not an error.
fn parse_image(bytes: &[u8]) -> Vec<Section> {
    match crate::ocr::image_text(bytes) {
        Ok(text) if letters(&text) >= MIN_IMAGE_TEXT => vec![Section { text, page: None }],
        _ => Vec::new(),
    }
}

fn pages_to_sections(pages: Vec<String>) -> Vec<Section> {
    pages
        .into_iter()
        .enumerate()
        .map(|(i, text)| Section {
            text,
            page: Some(i as u32 + 1),
        })
        .collect()
}

fn parse_pdf(bytes: &[u8]) -> Result<Vec<Section>, String> {
    // pdf-extract can panic on malformed files; contain it to this one file.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem_by_pages(bytes)
    }));
    let failure = match result {
        Ok(Ok(mut pages)) => {
            // Pages with no text layer are scans: read them off a rendering.
            let scans: Vec<u32> = pages
                .iter()
                .enumerate()
                .filter(|(_, text)| letters(text) < MIN_PAGE_TEXT)
                .map(|(i, _)| i as u32)
                .collect();
            if !scans.is_empty() && crate::ocr::available() {
                if let Ok(read) = crate::ocr::pdf_text(bytes, Some(&scans), MAX_OCR_PAGES) {
                    for (index, text) in read {
                        if letters(&text) >= MIN_PAGE_TEXT {
                            pages[index as usize] = text;
                        }
                    }
                }
            }
            return Ok(pages_to_sections(pages));
        }
        Ok(Err(err)) => format!("PDF could not be read: {err}"),
        Err(_) => "PDF parser failed on this file".to_string(),
    };
    // The text layer is unreadable, but the pages may still render.
    if crate::ocr::available() {
        if let Ok(read) = crate::ocr::pdf_text(bytes, None, MAX_OCR_PAGES) {
            let count = read.iter().map(|(index, _)| index + 1).max().unwrap_or(0) as usize;
            let mut pages = vec![String::new(); count];
            for (index, text) in read {
                pages[index as usize] = text;
            }
            if pages.iter().any(|text| letters(text) >= MIN_PAGE_TEXT) {
                return Ok(pages_to_sections(pages));
            }
        }
    }
    Err(failure)
}

// ----------------------------- docx / pptx -------------------------------

fn zip_entry(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<String, String> {
    let mut file = archive
        .by_name(name)
        .map_err(|e| format!("missing {name}: {e}"))?;
    let mut xml = String::new();
    file.read_to_string(&mut xml).map_err(|e| e.to_string())?;
    Ok(xml)
}

/// Collect the text of every `text_tag` element, adding a newline at the end
/// of each `para_tag`, plus tabs / line breaks.
fn xml_text(xml: &str, text_tag: &[u8], para_tag: &[u8]) -> Result<String, String> {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if e.name().as_ref() == text_tag {
                    in_text = true;
                }
            }
            Ok(Event::End(e)) => {
                let name = e.name();
                if name.as_ref() == text_tag {
                    in_text = false;
                } else if name.as_ref() == para_tag {
                    out.push('\n');
                }
            }
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                b"w:tab" => out.push('\t'),
                b"w:br" | b"a:br" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if in_text {
                    out.push_str(&t.unescape().unwrap_or_default());
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(format!("XML error: {err}")),
            _ => {}
        }
    }
    Ok(out)
}

fn parse_docx(bytes: &[u8]) -> Result<String, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a valid .docx: {e}"))?;
    let xml = zip_entry(&mut archive, "word/document.xml")?;
    xml_text(&xml, b"w:t", b"w:p")
}

fn parse_pptx(bytes: &[u8]) -> Result<Vec<Section>, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a valid .pptx: {e}"))?;
    // ppt/slides/slide12.xml -> 12
    let mut slides: Vec<(u32, String)> = archive
        .file_names()
        .filter_map(|name| {
            let n = name
                .strip_prefix("ppt/slides/slide")?
                .strip_suffix(".xml")?
                .parse::<u32>()
                .ok()?;
            Some((n, name.to_string()))
        })
        .collect();
    slides.sort();
    let mut sections = Vec::new();
    for (n, name) in slides {
        let xml = zip_entry(&mut archive, &name)?;
        sections.push(Section {
            text: xml_text(&xml, b"a:t", b"a:p")?,
            page: Some(n),
        });
    }
    Ok(sections)
}

// ------------------------------- sheets ----------------------------------

fn parse_sheet(bytes: &[u8]) -> Result<Vec<Section>, String> {
    use calamine::Reader as _;
    let mut workbook = calamine::open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|e| format!("spreadsheet could not be read: {e}"))?;
    let names = workbook.sheet_names().to_vec();
    let mut sections = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let Ok(range) = workbook.worksheet_range(name) else {
            continue;
        };
        let mut text = format!("Sheet: {name}\n");
        for row in range.rows() {
            let cells: Vec<String> = row
                .iter()
                .map(|c| c.to_string())
                .filter(|c| !c.trim().is_empty())
                .collect();
            if !cells.is_empty() {
                text.push_str(&cells.join(" | "));
                text.push('\n');
            }
        }
        sections.push(Section {
            text,
            page: Some(i as u32 + 1),
        });
    }
    Ok(sections)
}

// ------------------------------ fixtures ---------------------------------

/// In-memory sample documents, shared by the parser and indexer tests.
#[cfg(test)]
pub mod fixtures {
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    fn zip_of(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    pub fn docx(paragraphs: &[&str]) -> Vec<u8> {
        let body: String = paragraphs
            .iter()
            .map(|p| format!("<w:p><w:r><w:t xml:space=\"preserve\">{p}</w:t></w:r></w:p>"))
            .collect();
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}</w:body></w:document>"
        );
        zip_of(&[("word/document.xml", &xml)])
    }

    pub fn pptx(slides: &[&str]) -> Vec<u8> {
        let xmls: Vec<(String, String)> = slides
            .iter()
            .enumerate()
            .map(|(i, text)| {
                (
                    format!("ppt/slides/slide{}.xml", i + 1),
                    format!(
                        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><p:sld xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"
                    ),
                )
            })
            .collect();
        let entries: Vec<(&str, &str)> = xmls.iter().map(|(n, x)| (n.as_str(), x.as_str())).collect();
        zip_of(&entries)
    }

    /// One sheet named `sheet`, rows of string cells.
    pub fn xlsx(sheet: &str, rows: &[&[&str]]) -> Vec<u8> {
        let mut data = String::new();
        for (r, row) in rows.iter().enumerate() {
            data.push_str(&format!("<row r=\"{}\">", r + 1));
            for (c, cell) in row.iter().enumerate() {
                let col = (b'A' + c as u8) as char;
                data.push_str(&format!(
                    "<c r=\"{col}{}\" t=\"inlineStr\"><is><t>{cell}</t></is></c>",
                    r + 1
                ));
            }
            data.push_str("</row>");
        }
        let workbook = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"{sheet}\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>"
        );
        let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>";
        let sheet_xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{data}</sheetData></worksheet>"
        );
        let content_types = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>";
        let root_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>";
        zip_of(&[
            ("[Content_Types].xml", content_types),
            ("_rels/.rels", root_rels),
            ("xl/workbook.xml", &workbook),
            ("xl/_rels/workbook.xml.rels", rels),
            ("xl/worksheets/sheet1.xml", &sheet_xml),
        ])
    }

    /// A minimal single-page PDF containing `text` (ASCII, no parentheses).
    pub fn pdf(text: &str) -> Vec<u8> {
        let content = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref_at = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension() {
        assert_eq!(kind_for(Path::new("a/notes.MD")), Some(Kind::Text));
        assert_eq!(kind_for(Path::new("Form1.frm")), Some(Kind::Text));
        assert_eq!(kind_for(Path::new("report.pdf")), Some(Kind::Pdf));
        assert_eq!(kind_for(Path::new("x.docx")), Some(Kind::Docx));
        assert_eq!(kind_for(Path::new("x.pptx")), Some(Kind::Pptx));
        assert_eq!(kind_for(Path::new("x.xlsx")), Some(Kind::Sheet));
        assert_eq!(kind_for(Path::new("photo.png")), Some(Kind::Image));
        assert_eq!(kind_for(Path::new("movie.mp4")), None);
        assert_eq!(kind_for(Path::new("noext")), None);
    }

    #[test]
    fn decodes_text_encodings() {
        assert_eq!(decode_text(b"plain").as_deref(), Some("plain"));
        assert_eq!(decode_text(&[0xEF, 0xBB, 0xBF, b'h', b'i']).as_deref(), Some("hi"));
        // UTF-16 LE with BOM ("hi")
        assert_eq!(decode_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]).as_deref(), Some("hi"));
        // Windows-1252 pound sign + smart quote
        assert_eq!(decode_text(&[0xA3, b'5', 0x92]).as_deref(), Some("£5’"));
        // binary
        assert_eq!(decode_text(&[1, 2, 0, 3]), None);
    }

    #[test]
    fn cleans_whitespace() {
        assert_eq!(clean("a  \r\n\r\n\r\n\r\nb\0\n"), "a\n\nb");
    }

    #[test]
    fn parses_docx() {
        let bytes = fixtures::docx(&["Quarterly report", "Revenue grew &amp; costs fell."]);
        let sections = parse(Kind::Docx, &bytes).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].text, "Quarterly report\nRevenue grew & costs fell.");
    }

    #[test]
    fn parses_pptx_per_slide() {
        let bytes = fixtures::pptx(&["Welcome slide", "Roadmap for next year"]);
        let sections = parse(Kind::Pptx, &bytes).unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0], Section { text: "Welcome slide".into(), page: Some(1) });
        assert_eq!(sections[1].page, Some(2));
    }

    #[test]
    fn parses_xlsx_rows() {
        let bytes = fixtures::xlsx("Budget", &[&["Item", "Cost"], &["Laptop", "1200"]]);
        let sections = parse(Kind::Sheet, &bytes).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].page, Some(1));
        assert_eq!(sections[0].text, "Sheet: Budget\nItem | Cost\nLaptop | 1200");
    }

    #[test]
    fn parses_pdf_text() {
        let bytes = fixtures::pdf("Hello PDF from Loft");
        let sections = parse(Kind::Pdf, &bytes).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].page, Some(1));
        assert!(sections[0].text.contains("Hello PDF from Loft"), "got {:?}", sections[0].text);
    }

    #[test]
    fn bad_documents_error_instead_of_panicking() {
        assert!(parse(Kind::Docx, b"not a zip").is_err());
        assert!(parse(Kind::Pdf, b"not a pdf").is_err());
        assert!(parse(Kind::Sheet, b"nope").is_err());
        // binary "text" is simply not indexable
        assert!(parse(Kind::Text, &[0, 1, 2]).unwrap().is_empty());
        // and so is a picture that cannot be decoded
        assert!(parse(Kind::Image, b"not an image").unwrap().is_empty());
    }

    #[test]
    fn pictures_are_a_kind_of_their_own() {
        for name in ["scan.png", "Photo.JPG", "page.jpeg", "fax.tiff", "shot.bmp"] {
            assert_eq!(kind_for(Path::new(name)), Some(Kind::Image), "{name}");
        }
        assert_eq!(kind_for(Path::new("icon.svg")), None);
        assert_eq!(letters("  --- 12 ab\n"), 4);
    }

    fn sample_scan(name: &str) -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("sample-docs")
            .join("scans")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// The sample "scans" have no text in them at all, only pictures of text.
    #[test]
    fn scans_are_read_by_ocr() {
        if !crate::ocr::available() {
            eprintln!("skip: no text recognition on this system");
            return;
        }
        let started = std::time::Instant::now();
        let note = parse(Kind::Image, &sample_scan("delivery-note.png")).unwrap();
        assert_eq!(note.len(), 1);
        assert_eq!(note[0].page, None);
        let text = &note[0].text;
        eprintln!("delivery-note.png ({:.2}s):\n{text}", started.elapsed().as_secs_f32());
        for expected in ["BeanWorks", "Tuesday and Friday", "HC-20418"] {
            assert!(text.contains(expected), "missing {expected:?} in {text:?}");
        }

        // A scanned PDF: pdf-extract finds two empty pages, OCR fills them in.
        let bytes = sample_scan("lease-letter-scan.pdf");
        let empty = pdf_extract::extract_text_from_mem_by_pages(&bytes).unwrap();
        assert_eq!(empty.len(), 2);
        assert!(empty.iter().all(|page| letters(page) == 0), "the sample PDF should have no text layer");

        let started = std::time::Instant::now();
        let pages = parse(Kind::Pdf, &bytes).unwrap();
        eprintln!("lease-letter-scan.pdf ({:.2}s): {pages:#?}", started.elapsed().as_secs_f32());
        assert_eq!(pages.len(), 2);
        assert_eq!((pages[0].page, pages[1].page), (Some(1), Some(2)));
        assert!(pages[0].text.contains("30 September 2027"), "{:?}", pages[0].text);
        assert!(pages[0].text.contains("2,450"), "{:?}", pages[0].text);
        assert!(pages[1].text.contains("90 days"), "{:?}", pages[1].text);
        assert!(pages[1].text.contains("Helena Marsh"), "{:?}", pages[1].text);

        // A PDF that has real text is left exactly as it was (no OCR pass).
        let typed = parse(Kind::Pdf, &fixtures::pdf("Hello PDF from Loft, typed not scanned")).unwrap();
        assert!(typed[0].text.contains("typed not scanned"));
    }
}
