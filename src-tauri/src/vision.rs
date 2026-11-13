//! Picture descriptions: an optional small vision model writes a sentence or
//! two about each picture, so that pictures can be found by what they show
//! and not only by the text printed on them (which OCR already reads).
//!
//! The model is a llama-server started with `--mmproj`. It is started the
//! first time a picture needs describing during an indexing run and stopped
//! when the run ends, so it costs no memory the rest of the time.
//!
//! Small vision models get the gist right ("a lake with snow-capped
//! mountains", "a bar chart with four coloured bars") and invent details when
//! they go on for long. So descriptions are kept short, cut back to their
//! opening sentences, and labelled as automatic wherever they are used.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::runtime::Server;

/// What the model is asked. Short, plain prompts work best on small models.
const PROMPT: &str = "What is in this picture? Mention the scene, objects, colours and any text.";
/// Cap on the description; it is then trimmed to whole sentences.
const MAX_TOKENS: u32 = 70;
/// Sentences kept from the model's reply.
const MAX_SENTENCES: usize = 2;
/// How long the model may take to load.
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// How long one picture may take.
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(120);

/// Starts the picture model on first use and describes pictures with it.
/// Dropping it stops the model.
pub struct Describer {
    start: Box<dyn Fn() -> Result<Server, String> + Send + Sync>,
    server: tokio::sync::Mutex<Option<Server>>,
    /// The model could not be started; do not try again during this run.
    gave_up: AtomicBool,
}

impl Describer {
    /// `start` spawns the model's server (it is called at most once).
    pub fn new(start: impl Fn() -> Result<Server, String> + Send + Sync + 'static) -> Self {
        Describer {
            start: Box::new(start),
            server: tokio::sync::Mutex::new(None),
            gave_up: AtomicBool::new(false),
        }
    }

    /// The port of the running model, starting it if this is the first call.
    async fn port(&self) -> Option<u16> {
        if self.gave_up.load(Ordering::Relaxed) {
            return None;
        }
        let mut server = self.server.lock().await;
        if let Some(running) = server.as_ref() {
            return Some(running.port);
        }
        let started = match (self.start)() {
            Ok(started) => started,
            Err(err) => {
                eprintln!("Loft: the picture model could not be started: {err}");
                self.gave_up.store(true, Ordering::Relaxed);
                return None;
            }
        };
        let port = started.port;
        if !crate::runtime::wait_healthy(port, START_TIMEOUT).await {
            eprintln!("Loft: the picture model did not load");
            self.gave_up.store(true, Ordering::Relaxed);
            return None; // `started` is dropped here, which stops the process
        }
        *server = Some(started);
        Some(port)
    }

    /// A short description of the picture in `bytes`, or `None` when the
    /// model is unavailable or had nothing usable to say.
    pub async fn describe(&self, bytes: &[u8], file_name: &str) -> Option<String> {
        let port = self.port().await?;
        match describe_image(port, bytes, mime_for(file_name)).await {
            Ok(text) => Some(text).filter(|t| !t.is_empty()),
            Err(err) => {
                eprintln!("Loft: could not describe {file_name}: {err}");
                None
            }
        }
    }
}

fn mime_for(file_name: &str) -> &'static str {
    let ext = file_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "tif" | "tiff" => "image/tiff",
        _ => "image/jpeg",
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Ask the vision server on `port` what the picture shows.
pub async fn describe_image(port: u16, bytes: &[u8], mime: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(DESCRIBE_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let body = serde_json::json!({
        "messages": [{
            "role": "user",
            "content": [
                { "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{}", base64(bytes)) } },
                { "type": "text", "text": PROMPT },
            ],
        }],
        "temperature": 0,
        "repeat_penalty": 1.15,
        "max_tokens": MAX_TOKENS,
    });
    let resp = client
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    let value: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let message = value["error"]["message"].as_str().unwrap_or("unknown error");
        return Err(format!("the picture model returned {status}: {message}"));
    }
    let text = value["choices"][0]["message"]["content"].as_str().unwrap_or_default();
    Ok(tidy_description(text))
}

/// Keep the opening of the model's reply: the first sentences, on one line,
/// stopping before it turns into a list or a "detailed breakdown" (which is
/// where small models start inventing things) and dropping a cut-off ending.
pub fn tidy_description(raw: &str) -> String {
    let mut text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    for marker in ["###", "**", " 1. ", " - ", "Here's ", "Here is ", "Here are "] {
        if let Some(at) = text.find(marker) {
            text.truncate(at);
        }
    }
    let mut kept = String::new();
    let mut sentences = 0;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        // A full stop ends a sentence, but not inside a number ("2.5").
        let next = text[i + c.len_utf8()..].chars().next();
        let ends = matches!(c, '.' | '!' | '?') && next.is_none_or(|n| n.is_whitespace());
        if ends {
            kept.push_str(&text[start..i + c.len_utf8()]);
            start = i + c.len_utf8();
            sentences += 1;
            if sentences == MAX_SENTENCES {
                break;
            }
        }
    }
    kept.trim().to_string()
}

/// The text a picture is indexed under: what the model says it shows (when
/// there is a description) and the text read off it by OCR (when there is any).
pub fn picture_text(description: Option<&str>, ocr_text: &str) -> String {
    let description = description.map(str::trim).filter(|d| !d.is_empty());
    let ocr_text = ocr_text.trim();
    match (description, ocr_text.is_empty()) {
        (Some(d), true) => format!("Picture. Automatic description (may be imprecise): {d}"),
        (Some(d), false) => {
            format!("Picture. Automatic description (may be imprecise): {d}\nText in the picture:\n{ocr_text}")
        }
        (None, _) => ocr_text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_are_cut_back_to_their_opening() {
        assert_eq!(
            tidy_description("A lake with snow-capped mountains.  The sun is low.\nThe sky is pink. There are tre"),
            "A lake with snow-capped mountains. The sun is low."
        );
        // The made-up "detailed breakdown" is dropped.
        assert_eq!(
            tidy_description("The image shows a bar chart with four coloured bars. Here's a detailed description:\n\n1. **Blue Bar**: bread, 40%"),
            "The image shows a bar chart with four coloured bars."
        );
        assert_eq!(
            tidy_description("A document titled \"BeanWorks\".\n\n### Header\n- At the top"),
            "A document titled \"BeanWorks\"."
        );
        // Numbers keep their decimal point; an unfinished sentence is dropped.
        assert_eq!(tidy_description("The bar reaches 2.5 units. It is the tal"), "The bar reaches 2.5 units.");
        assert_eq!(tidy_description("no full stop at all"), "");
        assert_eq!(tidy_description(""), "");
    }

    #[test]
    fn picture_text_labels_the_description() {
        assert_eq!(
            picture_text(Some("A bar chart."), ""),
            "Picture. Automatic description (may be imprecise): A bar chart."
        );
        assert_eq!(
            picture_text(Some("A delivery note."), "Deliveries arrive every Tuesday\n"),
            "Picture. Automatic description (may be imprecise): A delivery note.\nText in the picture:\nDeliveries arrive every Tuesday"
        );
        // Without a description a picture is indexed by its text alone, as before.
        assert_eq!(picture_text(None, "Just the text\n"), "Just the text");
        assert_eq!(picture_text(Some("  "), "Just the text"), "Just the text");
        assert_eq!(picture_text(None, ""), "");
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xFF, 0xFE, 0xFD]), "//79");
        assert_eq!(mime_for("Photo.PNG"), "image/png");
        assert_eq!(mime_for("scan.jpeg"), "image/jpeg");
    }

    /// Describes the sample pictures with the real model.
    ///   cargo test --lib vision::tests::describes_sample_pictures -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires the SmolVLM picture model and the bundled llama-server"]
    async fn describes_sample_pictures() {
        use std::path::PathBuf;
        let models = PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
            .join("com.loft.desktop")
            .join("models");
        let model = models.join("SmolVLM-500M-Instruct-Q8_0.gguf");
        let mmproj = models.join("mmproj-SmolVLM-500M-Instruct-f16.gguf");
        if !model.exists() || !mmproj.exists() {
            eprintln!("skip: picture model not found in {}", models.display());
            return;
        }
        let describer = Describer::new(move || {
            crate::runtime::Runtime::new().spawn_vision(&model, &mmproj, "smolvlm-500m-q8")
        });
        let scans = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("sample-docs").join("scans");
        for (file, expected) in [("sales-chart.png", "chart"), ("delivery-note.png", "")] {
            let bytes = std::fs::read(scans.join(file)).unwrap();
            let started = std::time::Instant::now();
            let text = describer.describe(&bytes, file).await;
            eprintln!("{file} ({:.1}s): {text:?}", started.elapsed().as_secs_f32());
            let text = text.expect("a description");
            assert!(text.ends_with(['.', '!', '?']), "whole sentences: {text:?}");
            assert!(text.to_lowercase().contains(expected), "{file}: expected {expected:?} in {text:?}");
        }
    }
}
