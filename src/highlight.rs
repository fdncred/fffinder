//! Syntax highlighting for code previews, with syntect's bundled grammars
//! (Rust, Python, JavaScript, C, Go, shell, and dozens more) and a light or
//! dark theme to match the window.

use gpui_kit::{FontStyle, FontWeight, HighlightStyle, Hsla, Rgba};
use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{self, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Lines longer than this (minified files) aren't highlighted: the
/// grammars get slow on them, and the colors don't help.
const LONGEST_LINE: usize = 2_000;

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn themes() -> &'static ThemeSet {
    static SET: OnceLock<ThemeSet> = OnceLock::new();
    SET.get_or_init(ThemeSet::load_defaults)
}

fn to_style(style: highlighting::Style) -> HighlightStyle {
    let c = style.foreground;
    let color: Hsla = Rgba {
        r: f32::from(c.r) / 255.,
        g: f32::from(c.g) / 255.,
        b: f32::from(c.b) / 255.,
        a: f32::from(c.a) / 255.,
    }
    .into();
    let font = style.font_style;
    HighlightStyle {
        color: Some(color),
        font_weight: font
            .contains(highlighting::FontStyle::BOLD)
            .then_some(FontWeight::BOLD),
        font_style: font
            .contains(highlighting::FontStyle::ITALIC)
            .then_some(FontStyle::Italic),
        ..Default::default()
    }
}

/// Colored spans covering `text`, the start of the file at `path`, or
/// `None` when no grammar knows the file or a line is too long.
pub fn highlight(
    text: &str,
    path: &Path,
    dark: bool,
) -> Option<Vec<(Range<usize>, HighlightStyle)>> {
    if text.lines().any(|line| line.len() > LONGEST_LINE) {
        return None;
    }
    let set = syntaxes();
    let by_name = |name: &str| set.find_syntax_by_extension(name);
    let syntax = path
        .extension()
        .and_then(|e| by_name(&e.to_string_lossy()))
        // Names like `Makefile` and `Dockerfile` are grammars' extensions too.
        .or_else(|| path.file_name().and_then(|n| by_name(&n.to_string_lossy())))
        .or_else(|| set.find_syntax_by_first_line(text.lines().next()?))
        .filter(|s| s.name != "Plain Text")?;
    let theme = &themes().themes[if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    }];
    let mut lines = HighlightLines::new(syntax, theme);
    let mut spans: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut at = 0;
    for line in LinesWithEndings::from(text) {
        for (style, piece) in lines.highlight_line(line, set).ok()? {
            let style = to_style(style);
            let range = at..at + piece.len();
            at = range.end;
            // Neighbors in the same style become one span.
            match spans.last_mut() {
                Some((last, prev)) if *prev == style && last.end == range.start => {
                    last.end = range.end
                }
                _ => spans.push((range, style)),
            }
        }
    }
    Some(spans)
}

#[cfg(test)]
mod tests {
    use super::highlight;
    use std::path::Path;

    #[test]
    fn highlights_known_languages() {
        let text = "fn main() {\n    let x = 1; // one\n}\n";
        let spans = highlight(text, Path::new("main.rs"), false).expect("Rust is known");
        assert_eq!(spans.first().map(|s| s.0.start), Some(0));
        assert_eq!(
            spans.last().map(|s| s.0.end),
            Some(text.len()),
            "spans cover the text"
        );
        assert!(spans.len() > 3, "more than one color");
        assert!(highlight("#!/bin/sh\necho hi\n", Path::new("run"), true).is_some());
        assert!(highlight("just words", Path::new("notes.txt"), false).is_none());
        let minified = format!("var a={};\n", "1,".repeat(3_000));
        assert!(highlight(&minified, Path::new("app.js"), false).is_none());
    }
}
