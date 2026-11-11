//! Text recognition (OCR) for images and for PDF pages that are scans.
//!
//! Windows only for now: it uses the OCR engine and the PDF renderer that ship
//! with Windows 10/11, so nothing is bundled or downloaded. The recognition
//! language is the user's display language (it needs that language's OCR
//! pack, which Windows installs for most languages). On other systems
//! `available()` is false and callers simply get no text.
//!
//! Everything here blocks; call it from a blocking thread.

/// Pages rendered for OCR are this many times their size at 96 dpi (2.0 =
/// 192 dpi, about what a document scanner produces).
#[cfg(windows)]
const RENDER_SCALE: f64 = 2.0;
/// Images narrower or shorter than this are icons, not documents.
#[cfg(windows)]
const MIN_SIDE: u32 = 40;

/// True when this system can recognise text (checked once).
pub fn available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(imp::available)
}

/// The text visible in an image file (png, jpg, bmp, gif, tiff…), line by
/// line. Empty when there is none.
pub fn image_text(bytes: &[u8]) -> Result<String, String> {
    imp::image_text(bytes)
}

/// The text of PDF pages, read from a rendering of each page. `pages` are
/// 0-based page numbers (`None` = every page); at most `limit` are read.
/// Returns `(page, text)` for each page read.
pub fn pdf_text(bytes: &[u8], pages: Option<&[u32]>, limit: usize) -> Result<Vec<(u32, String)>, String> {
    imp::pdf_text(bytes, pages, limit)
}

#[cfg(windows)]
mod imp {
    use super::{MIN_SIDE, RENDER_SCALE};
    use windows::core::Result as WinResult;
    use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
    use windows::Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat, BitmapTransform,
        ColorManagementMode, ExifOrientationMode,
    };
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::Storage::Streams::InMemoryRandomAccessStream;

    fn engine() -> WinResult<OcrEngine> {
        OcrEngine::TryCreateFromUserProfileLanguages()
    }

    pub fn available() -> bool {
        engine().is_ok()
    }

    fn message(e: windows::core::Error) -> String {
        format!("text recognition failed: {}", e.message())
    }

    /// An in-memory stream holding `bytes`, positioned at the start.
    fn stream_of(bytes: &[u8]) -> WinResult<InMemoryRandomAccessStream> {
        let buffer = CryptographicBuffer::CreateFromByteArray(bytes)?;
        let stream = InMemoryRandomAccessStream::new()?;
        stream.WriteAsync(&buffer)?.join()?;
        stream.Seek(0)?;
        Ok(stream)
    }

    /// Decode the image in `stream` and read its text.
    fn recognise(engine: &OcrEngine, stream: &InMemoryRandomAccessStream) -> WinResult<String> {
        let decoder = BitmapDecoder::CreateAsync(stream)?.join()?;
        let (width, height) = (decoder.PixelWidth()?, decoder.PixelHeight()?);
        if width < MIN_SIDE || height < MIN_SIDE {
            return Ok(String::new());
        }
        // The engine refuses images larger than its limit: scale those down.
        let transform = BitmapTransform::new()?;
        let max = OcrEngine::MaxImageDimension()?;
        if width > max || height > max {
            let scale = max as f64 / width.max(height) as f64;
            transform.SetScaledWidth(((width as f64 * scale) as u32).max(1))?;
            transform.SetScaledHeight(((height as f64 * scale) as u32).max(1))?;
            transform.SetInterpolationMode(BitmapInterpolationMode::Fant)?;
        }
        let bitmap = decoder
            .GetSoftwareBitmapTransformedAsync(
                BitmapPixelFormat::Bgra8,
                BitmapAlphaMode::Premultiplied,
                &transform,
                ExifOrientationMode::RespectExifOrientation,
                ColorManagementMode::DoNotColorManage,
            )?
            .join()?;
        let result = engine.RecognizeAsync(&bitmap)?.join()?;
        let lines = result.Lines()?;
        let mut text = String::new();
        for i in 0..lines.Size()? {
            text.push_str(&lines.GetAt(i)?.Text()?.to_string());
            text.push('\n');
        }
        Ok(text)
    }

    pub fn image_text(bytes: &[u8]) -> Result<String, String> {
        let engine = engine().map_err(message)?;
        let stream = stream_of(bytes).map_err(message)?;
        recognise(&engine, &stream).map_err(message)
    }

    pub fn pdf_text(bytes: &[u8], pages: Option<&[u32]>, limit: usize) -> Result<Vec<(u32, String)>, String> {
        let read = || -> WinResult<Vec<(u32, String)>> {
            let engine = engine()?;
            let stream = stream_of(bytes)?;
            let document = PdfDocument::LoadFromStreamAsync(&stream)?.join()?;
            let count = document.PageCount()?;
            let max = OcrEngine::MaxImageDimension()? as f64;
            let wanted: Vec<u32> = match pages {
                Some(list) => list.iter().copied().filter(|p| *p < count).collect(),
                None => (0..count).collect(),
            };
            let mut out = Vec::new();
            for index in wanted.into_iter().take(limit) {
                // One unreadable page must not lose the others.
                let text = (|| -> WinResult<String> {
                    let page = document.GetPage(index)?;
                    let size = page.Size()?; // in 1/96 inch
                    let longest = size.Width.max(size.Height).max(1.0) as f64;
                    let scale = RENDER_SCALE.min(max / longest);
                    let options = PdfPageRenderOptions::new()?;
                    options.SetDestinationWidth(((size.Width as f64 * scale).round() as u32).max(1))?;
                    options.SetDestinationHeight(((size.Height as f64 * scale).round() as u32).max(1))?;
                    let rendered = InMemoryRandomAccessStream::new()?;
                    page.RenderWithOptionsToStreamAsync(&rendered, &options)?.join()?;
                    rendered.Seek(0)?;
                    let text = recognise(&engine, &rendered);
                    let _ = page.Close();
                    text
                })()
                .unwrap_or_default();
                out.push((index, text));
            }
            Ok(out)
        };
        read().map_err(message)
    }
}

#[cfg(not(windows))]
mod imp {
    const UNAVAILABLE: &str = "text recognition is not available on this system yet";

    pub fn available() -> bool {
        false
    }
    pub fn image_text(_bytes: &[u8]) -> Result<String, String> {
        Err(UNAVAILABLE.into())
    }
    pub fn pdf_text(_bytes: &[u8], _pages: Option<&[u32]>, _limit: usize) -> Result<Vec<(u32, String)>, String> {
        Err(UNAVAILABLE.into())
    }
}
