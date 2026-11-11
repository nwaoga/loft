//! Open a cited source file for the user.
//!
//! Documents and pictures open in their default app. Anything else (code,
//! scripts, config, macro-enabled Office files) is only *revealed* in the file
//! manager, so that clicking a source can never run a script.
//!
//! A PDF source knows its page. Where the default PDF app is one we know how
//! to tell "open at page N" (browsers, Acrobat, SumatraPDF on Windows), it is
//! started that way; otherwise the PDF just opens.

use std::path::Path;
use std::process::Command;

/// File types that are safe to hand straight to the default app.
const OPEN_DIRECTLY: &[&str] = &[
    "pdf", "docx", "pptx", "xlsx", "xls", "ods", "txt", "md", "markdown", "rst", "log", "csv", "tsv",
    "png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff", "webp",
];

pub fn opens_directly(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .map(|e| OPEN_DIRECTLY.contains(&e.as_str()))
        .unwrap_or(false)
}

/// Open `path` in its default app (a PDF at `page`, 1-based, when its app
/// supports that), or show it in the file manager.
pub fn open_or_reveal(path: &Path, page: Option<u32>) -> Result<(), String> {
    if !path.is_file() {
        return Err("That file no longer exists.".into());
    }
    let mut cmd = if opens_directly(path) {
        page.and_then(|page| pdf_at_page_command(path, page))
            .unwrap_or_else(|| open_command(path))
    } else {
        reveal_command(path)
    };
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open the file: {e}"))
}

/// `file:///C:/Docs/My%20file.pdf` for a local path.
fn file_url(path: &Path) -> String {
    let mut url = String::from("file:///");
    for byte in path.to_string_lossy().replace('\\', "/").trim_start_matches('/').bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => url.push(byte as char),
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

/// Arguments that make the PDF app `app` (its file name) open `path` at
/// `page`; `None` for an app we do not know how to ask.
fn pdf_page_args(app: &str, path: &Path, page: u32) -> Option<Vec<String>> {
    let page = page.max(1);
    match app.to_ascii_lowercase().as_str() {
        // Browsers' built-in viewers take the page from the URL.
        "msedge.exe" | "chrome.exe" | "brave.exe" | "vivaldi.exe" | "opera.exe" | "firefox.exe" => {
            Some(vec![format!("{}#page={page}", file_url(path))])
        }
        "acrobat.exe" | "acrord32.exe" => Some(vec!["/A".into(), format!("page={page}"), path.display().to_string()]),
        "sumatrapdf.exe" => Some(vec!["-page".into(), page.to_string(), path.display().to_string()]),
        _ => None,
    }
}

/// The program registered to open PDFs (full path), if Windows names one.
#[cfg(target_os = "windows")]
fn default_pdf_app() -> Option<std::path::PathBuf> {
    use windows_sys::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_NONE, ASSOCSTR_EXECUTABLE};
    let extension: Vec<u16> = ".pdf\0".encode_utf16().collect();
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    // SAFETY: both strings are NUL-terminated and `len` is the buffer's size.
    let result = unsafe {
        AssocQueryStringW(
            ASSOCF_NONE,
            ASSOCSTR_EXECUTABLE,
            extension.as_ptr(),
            std::ptr::null(),
            buffer.as_mut_ptr(),
            &mut len,
        )
    };
    if result != 0 || len < 2 {
        return None;
    }
    let exe = String::from_utf16_lossy(&buffer[..len as usize - 1]);
    let exe = std::path::PathBuf::from(exe.trim_end_matches('\0'));
    exe.is_file().then_some(exe)
}

/// A command that opens the PDF at `path` on `page`, when the default PDF
/// app can be told to. Only PDFs: no other format carries a page here.
#[cfg(target_os = "windows")]
fn pdf_at_page_command(path: &Path, page: u32) -> Option<Command> {
    let is_pdf = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if !is_pdf {
        return None;
    }
    let app = default_pdf_app()?;
    let args = pdf_page_args(&app.file_name()?.to_string_lossy(), path, page)?;
    let mut cmd = Command::new(app);
    cmd.args(args);
    Some(cmd)
}

#[cfg(not(target_os = "windows"))]
fn pdf_at_page_command(_path: &Path, _page: u32) -> Option<Command> {
    None // `open` / `xdg-open` cannot pass a page number on
}

#[cfg(target_os = "windows")]
fn open_command(path: &Path) -> Command {
    let mut cmd = Command::new("explorer.exe");
    cmd.arg(path);
    cmd
}

#[cfg(target_os = "windows")]
fn reveal_command(path: &Path) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("explorer.exe");
    // explorer wants exactly: /select,"C:\path with spaces\file.rs"
    // (Windows paths cannot contain a double quote.)
    cmd.raw_arg(format!("/select,\"{}\"", path.display()));
    cmd
}

#[cfg(target_os = "macos")]
fn open_command(path: &Path) -> Command {
    let mut cmd = Command::new("open");
    cmd.arg(path);
    cmd
}

#[cfg(target_os = "macos")]
fn reveal_command(path: &Path) -> Command {
    let mut cmd = Command::new("open");
    cmd.arg("-R").arg(path);
    cmd
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_command(path: &Path) -> Command {
    let mut cmd = Command::new("xdg-open");
    cmd.arg(path);
    cmd
}

#[cfg(all(unix, not(target_os = "macos")))]
fn reveal_command(path: &Path) -> Command {
    // No portable "select this file" on Linux: open the containing folder.
    let mut cmd = Command::new("xdg-open");
    cmd.arg(path.parent().unwrap_or(path));
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_documents_open_directly() {
        for p in ["a.pdf", "Report.DOCX", "notes.md", "C:\\x\\budget.xlsx", "faq.txt", "scan.PNG", "photo.jpeg"] {
            assert!(opens_directly(Path::new(p)), "{p}");
        }
        for p in ["deploy.bat", "run.ps1", "hook.js", "macro.docm", "build.sh", "main.rs", "noext"] {
            assert!(!opens_directly(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn pdf_apps_are_asked_for_the_page() {
        let pdf = Path::new("C:\\Docs\\My report #2 (final).pdf");
        let url = "file:///C:/Docs/My%20report%20%232%20%28final%29.pdf";
        assert_eq!(file_url(pdf), url);
        assert_eq!(pdf_page_args("msedge.exe", pdf, 7), Some(vec![format!("{url}#page=7")]));
        assert_eq!(pdf_page_args("Chrome.EXE", pdf, 1), Some(vec![format!("{url}#page=1")]));
        assert_eq!(
            pdf_page_args("Acrobat.exe", pdf, 3),
            Some(vec!["/A".to_string(), "page=3".to_string(), pdf.display().to_string()])
        );
        assert_eq!(
            pdf_page_args("SumatraPDF.exe", pdf, 12),
            Some(vec!["-page".to_string(), "12".to_string(), pdf.display().to_string()])
        );
        // An app we do not know: the PDF is opened the ordinary way.
        assert_eq!(pdf_page_args("SomeViewer.exe", pdf, 3), None);
        // Page numbers start at 1.
        assert_eq!(pdf_page_args("msedge.exe", pdf, 0), Some(vec![format!("{url}#page=1")]));
    }

    /// Prints which app opens PDFs here and how Loft would start it.
    #[test]
    #[cfg(target_os = "windows")]
    fn default_pdf_app_is_found() {
        match default_pdf_app() {
            Some(app) => {
                let name = app.file_name().unwrap().to_string_lossy().into_owned();
                eprintln!("default PDF app: {name} -> page args: {:?}", pdf_page_args(&name, Path::new("C:\\x.pdf"), 2));
            }
            None => eprintln!("no classic default PDF app registered: PDFs open without a page"),
        }
    }
}
