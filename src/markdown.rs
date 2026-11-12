//! Source-preserving Markdown inspection and exact-heading section edits.

use std::ops::Range;
use std::path::Path;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::{Error, Result};

/// The line ending used by a Markdown document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }

    pub fn detect(source: &str) -> Result<Self> {
        let bytes = source.as_bytes();
        let mut lf = false;
        let mut crlf = false;
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'\r' => {
                    if bytes.get(index + 1) != Some(&b'\n') {
                        return Err(Error::invalid("Unsupported Markdown carriage return."));
                    }
                    crlf = true;
                    index += 1;
                }
                b'\n' => lf = true,
                _ => {}
            }
            index += 1;
        }
        if lf && crlf {
            return Err(Error::invalid("Mixed Markdown line endings."));
        }
        Ok(if crlf { Self::CrLf } else { Self::Lf })
    }

    pub fn normalize(self, source: &str) -> Result<String> {
        Self::detect(source)?;
        Ok(source.replace("\r\n", "\n").replace('\n', self.as_str()))
    }
}

/// Byte offsets into the original UTF-8 source, never character offsets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarkdownHeading {
    pub level: u8,
    pub text: String,
    pub start: usize,
    /// End of the heading line, before its line ending.
    pub body: usize,
    /// Start of the next heading at the same or a shallower level.
    pub end: usize,
}

#[derive(Clone, Debug)]
pub struct MarkdownDocument {
    source: String,
    line_ending: LineEnding,
    headings: Vec<MarkdownHeading>,
    opaque: Vec<Range<usize>>,
    blocks: Vec<Range<usize>>,
}

impl MarkdownDocument {
    pub fn parse(source: impl Into<String>) -> Result<Self> {
        let source = source.into();
        let line_ending = LineEnding::detect(&source)?;
        let mut headings = Vec::new();
        let mut opaque = Vec::new();
        let mut blocks = Vec::new();
        let mut depth = 0_usize;
        for (event, range) in Parser::new_ext(&source, Options::empty()).into_offset_iter() {
            match event {
                Event::Start(ref tag) => {
                    // Container spans may include consumed reference definitions
                    // between their actual rendered children.
                    if !matches!(tag, Tag::List(_) | Tag::Item) {
                        blocks.push(range.clone());
                    }
                    if let Tag::Heading { level, .. } = tag
                        && depth == 0
                    {
                        let start = source[..range.start].rfind('\n').map_or(0, |i| i + 1);
                        let body = source[start..]
                            .find('\n')
                            .map_or(source.len(), |i| start + i);
                        let body = if body > start && source.as_bytes()[body - 1] == b'\r' {
                            body - 1
                        } else {
                            body
                        };
                        if let Some(text) = atx_text(&source[start..body]) {
                            headings.push(MarkdownHeading {
                                level: *level as u8,
                                text: text.to_owned(),
                                start,
                                body,
                                end: source.len(),
                            });
                        }
                    }
                    if matches!(tag, Tag::CodeBlock(_) | Tag::BlockQuote(_) | Tag::HtmlBlock) {
                        opaque.push(range.clone());
                    }
                    depth += 1;
                }
                Event::End(_) => depth = depth.saturating_sub(1),
                Event::Html(_) | Event::InlineHtml(_) => opaque.push(range),
                _ => {}
            }
        }
        for index in 0..headings.len() {
            headings[index].end = headings[index + 1..]
                .iter()
                .find(|heading| heading.level <= headings[index].level)
                .map_or(source.len(), |heading| heading.start);
        }
        Ok(Self {
            source,
            line_ending,
            headings,
            opaque,
            blocks,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub fn headings(&self) -> &[MarkdownHeading] {
        &self.headings
    }

    pub fn section(&self, text: &str) -> Result<&MarkdownHeading> {
        self.find_section(|heading| heading.text == text)
    }

    pub fn find_section(
        &self,
        predicate: impl Fn(&MarkdownHeading) -> bool,
    ) -> Result<&MarkdownHeading> {
        let mut matches = self.headings.iter().filter(|heading| predicate(heading));
        let heading = matches
            .next()
            .ok_or_else(|| Error::invalid("Missing matching Markdown heading."))?;
        if matches.next().is_some() {
            return Err(Error::invalid("Ambiguous duplicate Markdown headings."));
        }
        Ok(heading)
    }

    pub fn body(&self, heading: &MarkdownHeading) -> &str {
        &self.source[heading.body..heading.end]
    }

    pub(crate) fn opaque_at(&self, offset: usize) -> bool {
        self.opaque.iter().any(|range| range.contains(&offset))
    }

    pub(crate) fn block_at(&self, offset: usize) -> bool {
        self.blocks.iter().any(|range| range.contains(&offset))
    }

    /// Remove exact prompt lines, without touching code, quotes, or HTML examples.
    pub fn without_prompt(&self, prompt: &str) -> (String, usize) {
        let mut result = String::with_capacity(self.source.len());
        let mut offset = 0;
        let mut count = 0;
        for line in self.source.split_inclusive('\n') {
            let text = line.trim_end_matches(['\r', '\n']);
            if text == prompt && !self.opaque_at(offset) {
                count += 1;
            } else {
                result.push_str(line);
            }
            offset += line.len();
        }
        (result, count)
    }
}

fn atx_text(line: &str) -> Option<&str> {
    let indent = line.bytes().take_while(|byte| *byte == b' ').count();
    if indent > 3 {
        return None;
    }
    let content = &line[indent..];
    let level = content.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &content[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let text = rest.trim_matches([' ', '\t']);
    let without_hashes = text.trim_end_matches('#');
    let text = if without_hashes.len() != text.len()
        && (without_hashes.is_empty() || without_hashes.ends_with([' ', '\t']))
    {
        without_hashes.trim_end_matches([' ', '\t'])
    } else {
        text
    };
    Some(text)
}

/// Whether Markdown contains a note rather than only document structure.
///
/// Heading text, HTML, link definitions, thematic breaks, and the standard
/// contribution prompt do not count. Code and images with alt text do.
pub fn has_meaningful_notes(source: &str) -> Result<bool> {
    let document = MarkdownDocument::parse(source)?;
    let (source, _) = document.without_prompt("- Your contribution here.");
    let mut headings = 0_usize;
    let mut html = 0_usize;
    for event in Parser::new_ext(&source, Options::empty()) {
        match event {
            Event::Start(Tag::Heading { .. }) => headings += 1,
            Event::End(TagEnd::Heading(_)) => headings = headings.saturating_sub(1),
            Event::Start(Tag::HtmlBlock) => html += 1,
            Event::End(TagEnd::HtmlBlock) => html = html.saturating_sub(1),
            Event::Text(text)
            | Event::Code(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text)
                if headings == 0 && html == 0 && !text.trim().is_empty() =>
            {
                return Ok(true);
            }
            _ => {}
        }
    }
    Ok(false)
}

/// An editor scoped to one complete, exact ATX heading line.
#[derive(Clone, Debug)]
pub struct MarkdownSection {
    heading: String,
}

impl MarkdownSection {
    pub fn new(heading: impl Into<String>) -> Result<Self> {
        let heading = heading.into();
        if heading.contains(['\r', '\n']) || atx_text(&heading).is_none_or(str::is_empty) {
            return Err(Error::invalid(
                "Expected a complete single-line ATX heading.",
            ));
        }
        Ok(Self { heading })
    }

    pub fn locate(&self, source: &str) -> Result<MarkdownHeading> {
        let document = MarkdownDocument::parse(source)?;
        Ok(document
            .find_section(|heading| source[heading.start..heading.body] == self.heading)?
            .clone())
    }

    pub fn replace(&self, source: &str, body: &str) -> Result<String> {
        let current = self.locate(source)?;
        let ending = LineEnding::detect(source)?;
        let normalized = ending.normalize(body)?;
        let text = normalized.trim_matches(['\r', '\n']);
        let newline = ending.as_str();
        let replacement = if text.is_empty() {
            newline.repeat(2)
        } else {
            let tail = if current.end < source.len() {
                newline.repeat(2)
            } else if source.ends_with(newline) {
                newline.to_owned()
            } else {
                String::new()
            };
            format!("{newline}{newline}{text}{tail}")
        };
        Ok(replace_range(
            source,
            current.body..current.end,
            &replacement,
        ))
    }

    /// Replace the first literal occurrence in this section's body only.
    pub fn replace_text(&self, source: &str, search: &str, replacement: &str) -> Result<String> {
        let current = self.locate(source)?;
        let ending = LineEnding::detect(source)?;
        let replacement = ending.normalize(replacement)?;
        let search = ending.normalize(search)?;
        let body = source[current.body..current.end].replacen(&search, &replacement, 1);
        LineEnding::detect(&body)?;
        Ok(replace_range(source, current.body..current.end, &body))
    }

    /// Replace every literal occurrence in this section's body only.
    pub fn replace_all_text(
        &self,
        source: &str,
        search: &str,
        replacement: &str,
    ) -> Result<String> {
        let current = self.locate(source)?;
        let ending = LineEnding::detect(source)?;
        let replacement = ending.normalize(replacement)?;
        let search = ending.normalize(search)?;
        let body = source[current.body..current.end].replace(&search, &replacement);
        LineEnding::detect(&body)?;
        Ok(replace_range(source, current.body..current.end, &body))
    }

    /// Globally substitute a Rust regex within the section, with `$name` captures.
    pub fn replace_regex(&self, source: &str, pattern: &str, replacement: &str) -> Result<String> {
        let current = self.locate(source)?;
        let ending = LineEnding::detect(source)?;
        let replacement = ending.normalize(replacement)?;
        let pattern = regex::Regex::new(pattern)
            .map_err(|error| Error::invalid(format!("Invalid section regex: {error}")))?;
        let body = pattern.replace_all(&source[current.body..current.end], replacement.as_str());
        LineEnding::detect(&body)?;
        Ok(replace_range(source, current.body..current.end, &body))
    }

    pub fn validate_file(&self, path: &Path) -> Result<()> {
        let source = std::fs::read_to_string(path)?;
        self.locate(&source)?;
        Ok(())
    }

    pub fn replace_file(&self, path: &Path, body: &str) -> Result<()> {
        let source = std::fs::read_to_string(path)?;
        let updated = self.replace(&source, body)?;
        write_changed(path, &source, &updated)
    }

    pub fn replace_text_file(&self, path: &Path, search: &str, replacement: &str) -> Result<()> {
        let source = std::fs::read_to_string(path)?;
        let updated = self.replace_text(&source, search, replacement)?;
        write_changed(path, &source, &updated)
    }

    pub fn replace_all_text_file(
        &self,
        path: &Path,
        search: &str,
        replacement: &str,
    ) -> Result<()> {
        let source = std::fs::read_to_string(path)?;
        let updated = self.replace_all_text(&source, search, replacement)?;
        write_changed(path, &source, &updated)
    }

    pub fn replace_regex_file(&self, path: &Path, pattern: &str, replacement: &str) -> Result<()> {
        let source = std::fs::read_to_string(path)?;
        let updated = self.replace_regex(&source, pattern, replacement)?;
        write_changed(path, &source, &updated)
    }
}

pub(crate) fn replace_range(source: &str, range: Range<usize>, value: &str) -> String {
    let mut updated = String::with_capacity(source.len() - range.len() + value.len());
    updated.push_str(&source[..range.start]);
    updated.push_str(value);
    updated.push_str(&source[range.end..]);
    updated
}

pub(crate) fn write_changed(path: &Path, previous: &str, updated: &str) -> Result<()> {
    if previous != updated {
        std::fs::write(path, updated)
            .map_err(|error| Error::from(error).context(format!("Writing {}", path.display())))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_uses_real_top_level_atx_boundaries_and_byte_offsets() {
        let source = "# Café 🦀\n\n```\n## Target\n```\n\n> ## Target\n\n- ## Target\n\n<div>\n## Target\n</div>\n\nSetext\n------\n\n## Target\n\n雪\n\n### Child\n\nNested\n\n## History\n\nold\n";
        let section = MarkdownSection::new("## Target").unwrap();
        let current = section.locate(source).unwrap();
        assert_eq!(&source[current.start..current.body], "## Target");
        assert!(source[current.body..current.end].contains("### Child"));
        let updated = section.replace(source, "New 🦀").unwrap();
        assert!(updated.starts_with(&source[..current.body]));
        assert!(updated.ends_with("## History\n\nold\n"));
        assert!(!updated.contains("### Child"));
        assert!(updated.contains("## Target\n\nNew 🦀\n\n## History"));
    }

    #[test]
    fn exact_headings_and_ambiguous_documents_fail() {
        assert!(MarkdownSection::new("Target").is_err());
        assert!(MarkdownSection::new("## ").is_err());
        assert!(MarkdownSection::new("## Target\n").is_err());
        let section = MarkdownSection::new("## Target").unwrap();
        for source in ["# Target\n", "## Target ##\n", "## Target\n\n## Target\n"] {
            assert!(section.replace(source, "new").is_err());
        }
        assert!(MarkdownDocument::parse("# A\r\n\n").is_err());
        assert!(MarkdownDocument::parse("# A\rtext").is_err());
    }

    #[test]
    fn replacement_is_scoped_and_preserves_crlf_and_eof() {
        let section = MarkdownSection::new("## Target").unwrap();
        let source = "# 😀\r\n\r\n## Target\r\n\r\nold old\r\n\r\n## Other\r\nold";
        assert_eq!(
            section.replace_text(source, "old", "new\nline").unwrap(),
            "# 😀\r\n\r\n## Target\r\n\r\nnew\r\nline old\r\n\r\n## Other\r\nold"
        );
        assert_eq!(
            section.replace("## Target\n\nold", "new\n").unwrap(),
            "## Target\n\nnew"
        );
        assert!(section.replace(source, "new\r\nmixed\n").is_err());
        assert_eq!(
            section.replace("## Target\nold\n", "").unwrap(),
            "## Target\n\n"
        );
    }

    #[test]
    fn meaningful_notes_ignore_structure_but_accept_code_and_described_images() {
        for source in [
            "",
            " \n",
            "## Heading\n\n---\n\n<!-- comment -->\n\n[id]: https://example.com\n",
            "- Your contribution here.\n",
            "<div>\nOnly HTML\n</div>\n",
            "![](https://example.com/image.png)",
        ] {
            assert!(!has_meaningful_notes(source).unwrap(), "{source:?}");
        }
        for source in [
            "A real note.",
            "```\n## Example\n```",
            "`code`",
            "![Description](https://example.com/image.png)",
        ] {
            assert!(has_meaningful_notes(source).unwrap(), "{source:?}");
        }
    }

    #[test]
    fn global_substitutions_preserve_section_boundaries_and_capture_references() -> Result {
        let section = MarkdownSection::new("## Stable")?;
        let source = "# Versions\r\n\r\n## Stable\r\n\r\nv0.1.0 and v0.2.0\r\n\r\n### Nested\r\nv0.3.0\r\n\r\n## Older\r\nv0.1.0";
        assert_eq!(
            section.replace_regex(source, r"v(\d+)\.\d+\.\d+", "v${1}.9.0")?,
            "# Versions\r\n\r\n## Stable\r\n\r\nv0.9.0 and v0.9.0\r\n\r\n### Nested\r\nv0.9.0\r\n\r\n## Older\r\nv0.1.0"
        );
        assert_eq!(
            section.replace_all_text(source, "v0.", "v1.")?,
            "# Versions\r\n\r\n## Stable\r\n\r\nv1.1.0 and v1.2.0\r\n\r\n### Nested\r\nv1.3.0\r\n\r\n## Older\r\nv0.1.0"
        );
        assert_eq!(section.replace_regex(source, "absent", "new")?, source);
        assert!(section.replace_regex(source, "[", "new").is_err());
        assert!(section.replace_regex(source, "v0", "bad\r").is_err());
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("README.md");
        std::fs::write(&path, source)?;
        assert!(section.replace_regex_file(&path, "[", "new").is_err());
        assert_eq!(std::fs::read_to_string(&path)?, source);
        section.replace_regex_file(&path, r"v0\.\d+\.\d+", "stable")?;
        section.replace_all_text_file(&path, "stable", "ready\nnow")?;
        let updated = std::fs::read_to_string(path)?;
        assert_eq!(updated.matches("ready\r\nnow").count(), 3);
        assert!(updated.ends_with("## Older\r\nv0.1.0"));
        Ok(())
    }
    #[test]
    fn prompts_in_opaque_examples_are_untouched() {
        let source = "- Your contribution here.\n\n```\n- Your contribution here.\n```\n\n> - Your contribution here.\n\n<div>\n- Your contribution here.\n</div>\n\n- Your contribution here.\n";
        let (cleaned, count) = MarkdownDocument::parse(source)
            .unwrap()
            .without_prompt("- Your contribution here.");
        assert_eq!(count, 2);
        assert!(cleaned.contains("```\n- Your contribution here.\n```"));
        assert!(cleaned.contains("> - Your contribution here."));
        assert!(cleaned.contains("<div>\n- Your contribution here.\n</div>"));
    }

    #[test]
    fn file_edits_validate_before_writing() {
        let directory = tempfile::tempdir_in(".").unwrap();
        let path = directory.path().join("README.md");
        std::fs::write(&path, "## Target\n\nold\n").unwrap();
        let section = MarkdownSection::new("## Target").unwrap();
        section.validate_file(&path).unwrap();
        assert!(section.replace_file(&path, "bad\r").is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "## Target\n\nold\n"
        );
        section.replace_text_file(&path, "old", "new").unwrap();
        section.replace_file(&path, "final").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "## Target\n\nfinal\n"
        );
    }
}
