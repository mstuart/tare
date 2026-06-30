//! Opt-in LOSSY HTML compaction for LLM context windows.
//!
//! Removes `<script>`, `<style>`, and `<svg>` blocks; HTML comments `<!-- … -->`; and noisy
//! presentational attributes (style/class/data-*/on*). Collapses whitespace and drops empty lines.
//! Keeps text content and the semantic tag structure so an LLM can parse the page's meaning without
//! the rendering noise. Done with a scanner + regex over the string — no HTML-parser dependency.

use regex::Regex;

/// Compact HTML for LLM context: strip non-semantic noise, keep structure + text.
/// Returns `None` if the input is not HTML-ish or the result would not be smaller.
pub fn compact(html: &str) -> Option<String> {
    if !looks_like_html(html) {
        return None;
    }
    let mut out = html.to_string();

    // Remove block-level noise: <script>, <style>, <svg> (each may carry attributes on the
    // opening tag; content is arbitrary including newlines; closing tag may have whitespace).
    for tag in &["script", "style", "svg"] {
        // (?is): i = case-insensitive, s = dot matches newline (single-line mode).
        let re = Regex::new(&format!(r"(?is)<{tag}[^>]*>.*?</{tag}\s*>")).ok()?;
        out = re.replace_all(&out, "").into_owned();
    }

    // Remove HTML comments.
    let comment_re = Regex::new(r"(?s)<!--.*?-->").ok()?;
    out = comment_re.replace_all(&out, "").into_owned();

    // Strip noisy attributes from the remaining tags.
    out = strip_noisy_attributes(&out);

    // Collapse whitespace and drop empty lines.
    out = collapse_whitespace(&out);

    if out.len() < html.len() {
        Some(out)
    } else {
        None
    }
}

/// Heuristic: does this string look like HTML?
fn looks_like_html(s: &str) -> bool {
    let t = s.trim_start();
    (t.starts_with("<!") || t.starts_with('<'))
        && s.contains('>')
        && (s.contains("</") || s.contains("/>") || {
            let l = s.to_ascii_lowercase();
            l.contains("<html")
                || l.contains("<body")
                || l.contains("<div")
                || l.contains("<p>")
                || l.contains("<span")
                || l.contains("<table")
        })
}

/// Strip `style`, `class`, `data-*`, and `on*` (event handler) attributes from every tag in
/// `html`. Text between tags is untouched. Uses a two-level scan: find each `<…>` tag, then
/// remove noisy attributes inside it. Handles double-quoted, single-quoted, and unquoted values.
fn strip_noisy_attributes(html: &str) -> String {
    // Noisy attribute pattern: optional leading whitespace, the attribute name, optional value.
    let noisy = match Regex::new(
        r#"(?i)\s+(?:style|class|data-[^\s=>/]*|on[a-z][^\s=>/]*)(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>'"=`]+))?"#,
    ) {
        Ok(r) => r,
        Err(_) => return html.to_string(),
    };
    // Tag pattern: <…> (approximation; works for well-formed HTML).
    let tag_re = match Regex::new(r"<[^>]+>") {
        Ok(r) => r,
        Err(_) => return html.to_string(),
    };

    let mut out = String::with_capacity(html.len());
    let mut last = 0;
    for m in tag_re.find_iter(html) {
        // Append text content before this tag verbatim.
        out.push_str(&html[last..m.start()]);
        // Strip noisy attributes from the tag and emit the cleaned version.
        let cleaned = noisy.replace_all(m.as_str(), "");
        out.push_str(&cleaned);
        last = m.end();
    }
    out.push_str(&html[last..]);
    out
}

/// Collapse runs of whitespace within each line, then drop empty lines.
fn collapse_whitespace(html: &str) -> String {
    html.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <style>body { color: red; } .nav { display: none; }</style>
  <script type="text/javascript">
    window.onload = function() { alert('hello'); };
  </script>
  <title>Test Page</title>
</head>
<body>
  <!-- main navigation comment -->
  <nav class="nav" style="display:none" data-role="main" onclick="doNav()">
    <a href="/home" class="link">Home</a>
    <a href="/about" id="about-link">About</a>
  </nav>
  <main id="content">
    <h1 class="title" style="font-size:2em">Hello World</h1>
    <p>This is the visible text content.</p>
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
      <circle cx="50" cy="50" r="40"/>
    </svg>
    <p data-track="view">More visible text here.</p>
  </main>
</body>
</html>"#;

    #[test]
    fn removes_script_style_svg_comments_and_noisy_attrs() {
        let out = compact(SAMPLE).expect("should compact");
        // block noise removed
        assert!(
            !out.contains("window.onload"),
            "script block must be removed"
        );
        assert!(!out.contains("color: red"), "style block must be removed");
        assert!(!out.contains("<circle"), "svg block must be removed");
        // comment removed
        assert!(
            !out.contains("main navigation comment"),
            "comment must be removed"
        );
        // noisy attributes stripped
        assert!(!out.contains("class="), "class attr must be stripped");
        assert!(!out.contains("style="), "style attr must be stripped");
        assert!(
            !out.contains("data-role"),
            "data-role attr must be stripped"
        );
        assert!(!out.contains("onclick"), "onclick attr must be stripped");
        assert!(
            !out.contains("data-track"),
            "data-track attr must be stripped"
        );
        // text content and semantic structure kept
        assert!(out.contains("Hello World"), "heading text must survive");
        assert!(
            out.contains("visible text content"),
            "body text must survive"
        );
        assert!(
            out.contains("More visible text"),
            "more body text must survive"
        );
        assert!(out.contains("<h1"), "h1 tag structure must survive");
        assert!(out.contains("<nav"), "nav tag structure must survive");
        assert!(
            out.contains(r#"href="/home""#),
            "href attribute must survive"
        );
        assert!(
            out.contains(r#"id="about-link""#),
            "id attribute must survive"
        );
        // result is smaller
        assert!(
            out.len() < SAMPLE.len(),
            "output must be smaller than input"
        );
    }

    #[test]
    fn non_html_returns_none() {
        assert!(compact("just plain text here, no HTML tags at all").is_none());
        assert!(compact(r#"{"key": "value", "no": "html"}"#).is_none());
        assert!(compact("").is_none());
    }
}
