//! Image data-URI de-referencer.
//!
//! Inline base64 images in LLM context can be enormous (a screenshot is often
//! tens of thousands of tokens). [`deref`] replaces every `data:image/*;base64,…`
//! URI with a compact `[tare-image …]` marker and returns the originals keyed by
//! id so the transform is fully reversible.

use regex::Regex;
use std::sync::OnceLock;
use xxhash_rust::xxh3::xxh3_64;

/// Output of [`deref`]: cleaned text plus the list of extracted images.
pub struct Deref {
    pub text: String,
    pub images: Vec<DerefImage>,
}

/// A single extracted image.
pub struct DerefImage {
    /// 8-character hex id derived from xxh3 of the original data URI.
    pub id: String,
    /// The compact marker that was substituted into the text.
    pub marker: String,
    /// The original full `data:image/…;base64,…` URI.
    pub original: String,
}

fn image_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"data:image/([a-z+]+);base64,[A-Za-z0-9+/=]+").unwrap())
}

/// Replace every inline base64 image data URI in `text` with a compact marker.
///
/// Each marker has the form `[tare-image id=<8hex> fmt=<fmt> ~<N>KB]` where:
/// - `id` is the first 8 hex characters of the xxh3_64 hash of the original URI
/// - `fmt` is the image format (e.g. `png`, `jpeg`, `webp`, `svg+xml`)
/// - `N` is the estimated decoded size in KB (`base64_len * 3 / 4 / 1024`)
///
/// Returns `None` if there are no base64 image URIs in `text`.
pub fn deref(text: &str) -> Option<Deref> {
    let re = image_re();
    if !re.is_match(text) {
        return None;
    }

    let mut images = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut last_end = 0;

    for cap in re.captures_iter(text) {
        let m = cap.get(0).unwrap();
        let fmt = cap.get(1).unwrap().as_str();
        let original = m.as_str().to_string();

        // id: first 8 hex chars of xxh3_64 of the full data URI.
        let hash = xxh3_64(original.as_bytes());
        let id = format!("{:016x}", hash)[..8].to_string();

        // Size estimate: base64 encodes 3 bytes as 4 chars → decoded_bytes ≈ b64_len * 3 / 4.
        let b64_start = original.find(";base64,").unwrap() + ";base64,".len();
        let b64_len = original.len() - b64_start;
        let kb = b64_len * 3 / 4 / 1024;

        let marker = format!("[tare-image id={id} fmt={fmt} ~{kb}KB]");

        out.push_str(&text[last_end..m.start()]);
        out.push_str(&marker);
        last_end = m.end();

        images.push(DerefImage {
            id,
            marker,
            original,
        });
    }

    out.push_str(&text[last_end..]);

    Some(Deref { text: out, images })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal 1×1 transparent PNG in base64.
    const PNG1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    // A different minimal 1×1 PNG → different id.
    const PNG2: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8z8BQDwADhQGAWjR9awAAAABJRU5ErkJggg==";

    fn png_uri(b64: &str) -> String {
        format!("data:image/png;base64,{b64}")
    }

    #[test]
    fn two_embedded_pngs_replaced_and_reversible() {
        let uri1 = png_uri(PNG1);
        let uri2 = png_uri(PNG2);
        let doc = format!("before {uri1} middle {uri2} after");

        let d = deref(&doc).expect("should detect two images");

        // Both images are extracted.
        assert_eq!(d.images.len(), 2);

        // Text shrinks dramatically: two long URIs replaced by short markers.
        assert!(
            d.text.len() < doc.len() / 2,
            "cleaned text ({} bytes) should be less than half of original ({} bytes)",
            d.text.len(),
            doc.len(),
        );

        // Markers are present in cleaned text; raw base64 is gone.
        assert!(d.text.contains(&d.images[0].marker));
        assert!(d.text.contains(&d.images[1].marker));
        assert!(!d.text.contains("base64,"));

        // Originals are captured verbatim.
        assert_eq!(d.images[0].original, uri1);
        assert_eq!(d.images[1].original, uri2);

        // Ids are 8 hex chars and differ between distinct images.
        assert_eq!(d.images[0].id.len(), 8);
        assert!(d.images[0].id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(d.images[0].id, d.images[1].id);

        // Reversibility: replacing every marker with its original reproduces the document.
        let mut restored = d.text.clone();
        for img in &d.images {
            restored = restored.replace(&img.marker, &img.original);
        }
        assert_eq!(restored, doc);
    }

    #[test]
    fn no_images_returns_none() {
        assert!(deref("hello world, no images here").is_none());
        assert!(deref("").is_none());
        // Non-image data URI must not match.
        assert!(deref("data:text/plain;base64,aGVsbG8=").is_none());
    }
}
