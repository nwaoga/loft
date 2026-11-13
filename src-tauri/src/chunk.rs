//! M4 — chunker.
//!
//! Splits parsed sections into overlapping chunks sized for the embedding
//! model. Splits prefer natural boundaries (paragraph, line, sentence, word)
//! and only cut mid-word as a last resort. Sizes are in bytes of UTF-8 text
//! (~4 bytes per token for English prose, less for code), always cut on a
//! character boundary.

use crate::parse::Section;
use crate::store::ChunkInput;

/// Target maximum chunk size (~400 tokens of prose).
pub const CHUNK_MAX: usize = 1600;
/// Text carried over from the end of one chunk to the start of the next.
pub const CHUNK_OVERLAP: usize = 200;

const SEPARATORS: &[&str] = &["\n\n", "\n", ". ", " "];

/// Chunk every section, keeping each section's page number on its chunks.
pub fn chunk_sections(sections: &[Section], max: usize, overlap: usize) -> Vec<ChunkInput> {
    let mut out = Vec::new();
    for section in sections {
        for text in split_text(&section.text, max, overlap) {
            out.push(ChunkInput {
                text,
                page: section.page,
            });
        }
    }
    out
}

/// Split `text` into chunks of at most `max` bytes with ~`overlap` bytes of
/// shared context between neighbours.
pub fn split_text(text: &str, max: usize, overlap: usize) -> Vec<String> {
    let max = max.max(16);
    let overlap = overlap.min(max / 2);
    let pieces = split_pieces(text, max, SEPARATORS);

    let mut chunks = Vec::new();
    let mut current = String::new();
    for piece in pieces {
        if !current.is_empty() && current.len() + piece.len() > max {
            let tail = overlap_tail(&current, overlap);
            push_chunk(&mut chunks, &current);
            current = tail;
            // Never let the carried-over tail push a chunk past the limit.
            if current.len() + piece.len() > max {
                current.clear();
            }
        }
        current.push_str(&piece);
    }
    push_chunk(&mut chunks, &current);
    chunks
}

fn push_chunk(chunks: &mut Vec<String>, text: &str) {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        chunks.push(trimmed.to_string());
    }
}

/// Break text into pieces no longer than `max`, splitting on the coarsest
/// separator that works. Pieces keep their separators, so concatenating them
/// reproduces the input.
fn split_pieces(text: &str, max: usize, separators: &[&str]) -> Vec<String> {
    if text.len() <= max {
        return vec![text.to_string()];
    }
    if let Some((sep, finer)) = separators.split_first() {
        let parts: Vec<&str> = text.split_inclusive(sep).collect();
        if parts.len() > 1 {
            return parts
                .into_iter()
                .flat_map(|part| split_pieces(part, max, finer))
                .collect();
        }
        return split_pieces(text, max, finer);
    }
    // No separator left: hard split on character boundaries.
    let mut out = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        out.push(text[start..end].to_string());
        start = end;
    }
    out
}

/// The last ~`n` bytes of `text`, starting at a word boundary.
fn overlap_tail(text: &str, n: usize) -> String {
    if n == 0 || text.len() <= n {
        return String::new();
    }
    let mut start = text.len() - n;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let tail = &text[start..];
    match tail.find(char::is_whitespace) {
        Some(i) => tail[i..].trim_start().to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_one_chunk() {
        assert_eq!(split_text("hello world", 100, 20), vec!["hello world"]);
        assert!(split_text("   \n ", 100, 20).is_empty());
    }

    #[test]
    fn respects_max_and_prefers_paragraphs() {
        let para = "word ".repeat(30); // 150 bytes
        let text = format!("{para}\n\n{para}\n\n{para}");
        let chunks = split_text(&text, 200, 0);
        assert_eq!(chunks.len(), 3, "one paragraph per chunk");
        assert!(chunks.iter().all(|c| c.len() <= 200));
    }

    #[test]
    fn neighbours_overlap() {
        let text = (0..200).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ");
        let chunks = split_text(&text, 120, 30);
        assert!(chunks.len() > 2);
        assert!(chunks.iter().all(|c| c.len() <= 120));
        for pair in chunks.windows(2) {
            let last_word = pair[0].split_whitespace().last().unwrap();
            assert!(
                pair[1].split_whitespace().any(|w| w == last_word),
                "chunk should start with context from the previous one"
            );
        }
        // Nothing is lost: every word appears somewhere.
        for i in 0..200 {
            let w = format!("w{i}");
            assert!(chunks.iter().any(|c| c.split_whitespace().any(|x| x == w)));
        }
    }

    #[test]
    fn hard_splits_unbroken_text_on_char_boundaries() {
        let text = "é".repeat(500); // 1000 bytes, no separators
        let chunks = split_text(&text, 101, 0);
        assert!(chunks.iter().all(|c| c.len() <= 101));
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn sections_keep_their_page() {
        let sections = vec![
            Section { text: "page one text".into(), page: Some(1) },
            Section { text: "page two text".into(), page: Some(2) },
        ];
        let chunks = chunk_sections(&sections, 100, 10);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1].page, Some(2));
    }
}
