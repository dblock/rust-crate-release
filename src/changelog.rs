//! Handwritten and generated changelogs with source-preserving reconciliation.

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::Path;

use semver::Version;

use crate::markdown::{
    LineEnding, MarkdownDocument, MarkdownHeading, has_meaningful_notes, replace_range,
    write_changed,
};
use crate::{Error, Result};

/// A prepared document and the release notes captured from it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangelogEdit {
    pub source: String,
    pub notes: String,
}

/// A changelog is a pure transformation until its file helpers are called.
pub trait Changelog: Send + Sync {
    fn release(
        &self,
        source: &str,
        version: &Version,
        date: &str,
        generated_notes: Option<&str>,
    ) -> Result<ChangelogEdit>;

    fn next(&self, source: &str, released: &Version, next: &Version) -> Result<String>;

    fn allows_missing_file(&self) -> bool {
        false
    }

    fn release_file(
        &self,
        path: &Path,
        version: &Version,
        date: &str,
        generated_notes: Option<&str>,
    ) -> Result<ChangelogEdit> {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && self.allows_missing_file() =>
            {
                String::new()
            }
            Err(error) => return Err(error.into()),
        };
        let edit = self.release(&source, version, date, generated_notes)?;
        write_changed(path, &source, &edit.source)?;
        Ok(edit)
    }

    fn next_file(&self, path: &Path, released: &Version, next: &Version) -> Result<()> {
        let source = std::fs::read_to_string(path)?;
        let updated = self.next(&source, released, next)?;
        write_changed(path, &source, &updated)
    }
}

/// Validated template configuration, copied into a changelog instance.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChangelogTemplate {
    placeholder: Option<String>,
    sections: Vec<String>,
}

impl ChangelogTemplate {
    pub fn new(placeholder: Option<String>, sections: Vec<String>) -> Result<Self> {
        if placeholder
            .as_deref()
            .is_some_and(|text| !single_line(text))
        {
            return Err(Error::invalid(
                "Contribution prompt must be a nonempty single line.",
            ));
        }
        let mut unique = BTreeSet::new();
        if sections
            .iter()
            .any(|section| !single_line(section) || !unique.insert(section.as_str()))
        {
            return Err(Error::invalid(
                "Changelog categories must be unique nonempty single lines.",
            ));
        }
        Ok(Self {
            placeholder,
            sections,
        })
    }

    pub fn placeholder(&self) -> Option<&str> {
        self.placeholder.as_deref()
    }

    pub fn sections(&self) -> &[String] {
        &self.sections
    }
}

fn single_line(text: &str) -> bool {
    !text.trim().is_empty() && !text.contains(['\r', '\n'])
}

/// Intridea's numeric `(Next)` heading and required contribution prompt.
#[derive(Clone, Debug)]
pub struct IntrideaChangelog {
    template: ChangelogTemplate,
}

impl Default for IntrideaChangelog {
    fn default() -> Self {
        Self {
            template: ChangelogTemplate {
                placeholder: Some("- Your contribution here.".to_owned()),
                sections: Vec::new(),
            },
        }
    }
}

impl IntrideaChangelog {
    pub fn new(placeholder: impl Into<String>) -> Result<Self> {
        Ok(Self {
            template: ChangelogTemplate::new(Some(placeholder.into()), Vec::new())?,
        })
    }
}

/// Keep a Changelog is the default handwritten convention.
#[derive(Clone, Debug, Default)]
pub struct KeepAChangelog {
    template: ChangelogTemplate,
}

impl KeepAChangelog {
    pub fn new(template: ChangelogTemplate) -> Self {
        Self { template }
    }
}

/// Dated sections containing immutable, captured semantic notes.
#[derive(Clone, Debug, Default)]
pub struct GeneratedChangelog;

#[derive(Clone, Copy)]
enum Convention {
    Intridea,
    Keep,
}

impl Convention {
    fn pending(self, heading: &MarkdownHeading) -> bool {
        match self {
            Self::Intridea => heading.text.ends_with(" (Next)"),
            Self::Keep => heading.text == "[Unreleased]",
        }
    }

    fn release_candidate(self, text: &str, version: &Version) -> bool {
        match self {
            Self::Intridea => {
                text.starts_with(&format!("{version} (")) && !text.ends_with(" (Next)")
            }
            Self::Keep => {
                text == format!("[{version}]") || text.starts_with(&format!("[{version}] "))
            }
        }
    }

    fn dated(self, text: &str, version: &Version) -> bool {
        let date = match self {
            Self::Intridea => text
                .strip_prefix(&format!("{version} ("))
                .and_then(|date| date.strip_suffix(')'))
                .map(str::to_owned),
            Self::Keep => text
                .strip_prefix(&format!("[{version}] - "))
                .filter(|date| {
                    date.len() == 10 && date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-'
                })
                .map(|date| date.replace('-', "/")),
        };
        date.is_some_and(|date| validate_date(&date).is_ok())
    }

    fn released_title(self, version: &Version, date: &str) -> String {
        match self {
            Self::Intridea => format!("{version} ({date})"),
            Self::Keep => format!("[{version}] - {}", date.replace('/', "-")),
        }
    }

    fn next_title(self, next: &Version) -> String {
        match self {
            Self::Intridea => format!("{next} (Next)"),
            Self::Keep => "[Unreleased]".to_owned(),
        }
    }
}

impl Changelog for IntrideaChangelog {
    fn release(
        &self,
        source: &str,
        version: &Version,
        date: &str,
        generated_notes: Option<&str>,
    ) -> Result<ChangelogEdit> {
        release_handwritten(
            source,
            version,
            date,
            generated_notes,
            Convention::Intridea,
            &self.template,
        )
    }

    fn next(&self, source: &str, released: &Version, next: &Version) -> Result<String> {
        next_handwritten(source, released, next, Convention::Intridea, &self.template)
    }
}

impl Changelog for KeepAChangelog {
    fn release(
        &self,
        source: &str,
        version: &Version,
        date: &str,
        generated_notes: Option<&str>,
    ) -> Result<ChangelogEdit> {
        release_handwritten(
            source,
            version,
            date,
            generated_notes,
            Convention::Keep,
            &self.template,
        )
    }

    fn next(&self, source: &str, released: &Version, next: &Version) -> Result<String> {
        next_handwritten(source, released, next, Convention::Keep, &self.template)
    }
}

fn release_handwritten(
    source: &str,
    version: &Version,
    date: &str,
    captured: Option<&str>,
    convention: Convention,
    template: &ChangelogTemplate,
) -> Result<ChangelogEdit> {
    validate_stable(version)?;
    validate_date(date)?;
    let document = MarkdownDocument::parse(source)?;
    if matches!(convention, Convention::Keep) {
        references(&document)?;
    }
    let existing = release_section(&document, convention, version)?;
    if let Some(existing) = existing {
        let notes = clean_notes(document.body(existing), existing.level, template, false)?;
        let notes = notes_without_references(&notes)?;
        validate_notes(&notes, captured)?;
        return Ok(ChangelogEdit {
            source: source.to_owned(),
            notes: notes.trim().to_owned(),
        });
    }
    let current = document.find_section(|heading| convention.pending(heading))?;
    if matches!(convention, Convention::Intridea) {
        let pending_version = current
            .text
            .strip_suffix(" (Next)")
            .ok_or_else(|| Error::invalid("Invalid Next heading."))?;
        validate_stable(&Version::parse(pending_version)?)?;
    }
    let notes = clean_notes(
        document.body(current),
        current.level,
        template,
        matches!(convention, Convention::Intridea),
    )?;
    let captured_notes = notes_without_references(&notes)?;
    validate_notes(&captured_notes, captured)?;
    let heading = format!(
        "{} {}",
        "#".repeat(usize::from(current.level)),
        convention.released_title(version, date)
    );
    let updated = replace_range(
        source,
        current.start..current.end,
        &format!("{heading}{notes}"),
    );
    let updated = if matches!(convention, Convention::Keep) {
        update_comparison(&updated, version)?
    } else {
        updated
    };
    Ok(ChangelogEdit {
        source: updated,
        notes: captured_notes.trim().to_owned(),
    })
}

fn release_section<'a>(
    document: &'a MarkdownDocument,
    convention: Convention,
    version: &Version,
) -> Result<Option<&'a MarkdownHeading>> {
    let candidates: Vec<_> = document
        .headings()
        .iter()
        .filter(|heading| convention.release_candidate(&heading.text, version))
        .collect();
    if candidates.len() > 1 {
        return Err(Error::invalid("Duplicate release headings."));
    }
    if let Some(heading) = candidates.first() {
        if !convention.dated(&heading.text, version) {
            return Err(Error::invalid(
                "Existing release heading has an unsupported date or format.",
            ));
        }
        Ok(Some(heading))
    } else {
        Ok(None)
    }
}

fn clean_notes(
    source: &str,
    depth: u8,
    template: &ChangelogTemplate,
    required_prompt: bool,
) -> Result<String> {
    let mut notes = source.to_owned();
    if let Some(prompt) = template.placeholder() {
        let (cleaned, count) = MarkdownDocument::parse(&notes)?.without_prompt(prompt);
        if required_prompt && count == 0 {
            return Err(Error::invalid("Expected at least one contribution prompt."));
        }
        notes = cleaned;
    }
    if !required_prompt && (template.placeholder.is_some() || !template.sections.is_empty()) {
        let document = MarkdownDocument::parse(&notes)?;
        let mut edits = Vec::new();
        for heading in document
            .headings()
            .iter()
            .filter(|heading| heading.level == depth + 1)
        {
            if !has_meaningful_notes(document.body(heading))? {
                let end = heading.body
                    + notes[heading.body..]
                        .find('\n')
                        .map_or(0, |index| index + 1);
                edits.push(heading.start..end);
            }
        }
        for edit in edits.into_iter().rev() {
            notes.replace_range(edit, "");
        }
    }
    Ok(notes)
}

fn next_handwritten(
    source: &str,
    released: &Version,
    next: &Version,
    convention: Convention,
    template: &ChangelogTemplate,
) -> Result<String> {
    validate_stable(released)?;
    validate_stable(next)?;
    if next <= released {
        return Err(Error::invalid(
            "Next development version must exceed the release.",
        ));
    }
    let document = MarkdownDocument::parse(source)?;
    if matches!(convention, Convention::Keep) {
        let definitions = references(&document)?;
        if let Some(unreleased) = definitions
            .iter()
            .find(|reference| reference.label == "unreleased")
            && comparison(&unreleased.url)?.previous_version != *released
        {
            return Err(Error::invalid(
                "Unreleased comparison must start at the released version.",
            ));
        }
    }
    let current = release_section(&document, convention, released)?
        .ok_or_else(|| Error::invalid("Missing released changelog section."))?;
    let pending: Vec<_> = document
        .headings()
        .iter()
        .filter(|heading| convention.pending(heading))
        .collect();
    if !pending.is_empty() {
        if pending.len() == 1
            && pending[0].text == convention.next_title(next)
            && pending[0].level == current.level
            && pending[0].end == current.start
        {
            return Ok(source.to_owned());
        }
        return Err(Error::invalid(
            "Conflicting unreleased heading or position.",
        ));
    }
    let categories: Vec<_> = if matches!(convention, Convention::Intridea) {
        document
            .headings()
            .iter()
            .filter(|heading| {
                heading.level == current.level + 1
                    && heading.start >= current.body
                    && heading.start < current.end
            })
            .map(|heading| source[heading.start..heading.body].to_owned())
            .collect()
    } else {
        template
            .sections
            .iter()
            .map(|name| format!("{} {name}", "#".repeat(usize::from(current.level) + 1)))
            .collect()
    };
    if current.level == 6 && !categories.is_empty() {
        return Err(Error::invalid("Category headings cannot exceed level six."));
    }
    let newline = document.line_ending().as_str();
    let mut lines = vec![
        format!(
            "{} {}",
            "#".repeat(usize::from(current.level)),
            convention.next_title(next)
        ),
        String::new(),
    ];
    if categories.is_empty() {
        if let Some(prompt) = template.placeholder() {
            lines.extend([prompt.to_owned(), String::new()]);
        }
    } else {
        for category in categories {
            lines.extend([category, String::new()]);
            if let Some(prompt) = template.placeholder() {
                lines.extend([prompt.to_owned(), String::new()]);
            }
        }
    }
    Ok(replace_range(
        source,
        current.start..current.start,
        &(lines.join(newline) + newline),
    ))
}

impl Changelog for GeneratedChangelog {
    fn release(
        &self,
        source: &str,
        version: &Version,
        date: &str,
        generated_notes: Option<&str>,
    ) -> Result<ChangelogEdit> {
        validate_stable(version)?;
        validate_date(date)?;
        let notes = generated_notes.ok_or_else(|| {
            Error::invalid("Generated changelog requires captured release notes.")
        })?;
        validate_notes(notes, None)?;
        let initial;
        let source = if source.is_empty() {
            initial = "# Changelog\n\n".to_owned();
            &initial
        } else {
            source
        };
        let document = MarkdownDocument::parse(source)?;
        if let Some(existing) = release_section(&document, Convention::Intridea, version)? {
            let existing_notes = notes_without_references(document.body(existing))?;
            validate_notes(&existing_notes, Some(notes))?;
            return Ok(ChangelogEdit {
                source: source.to_owned(),
                notes: notes.trim().to_owned(),
            });
        }
        // A Next section for this version must not coexist with generated output.
        if document
            .headings()
            .iter()
            .any(|heading| heading.text == format!("{version} (Next)"))
        {
            return Err(Error::invalid(
                "Generated release conflicts with a Next heading.",
            ));
        }
        let position = document
            .headings()
            .iter()
            .find(|heading| heading.level == 2)
            .map_or(source.len(), |heading| heading.start);
        let newline = document.line_ending().as_str();
        let normalized = document.line_ending().normalize(notes)?;
        let prefix = &source[..position];
        let separator = if prefix.is_empty() || prefix.ends_with(&newline.repeat(2)) {
            String::new()
        } else if prefix.ends_with(newline) {
            newline.to_owned()
        } else {
            newline.repeat(2)
        };
        let entry = format!(
            "{separator}## {version} ({date}){newline}{newline}{}{newline}{newline}",
            normalized.trim()
        );
        Ok(ChangelogEdit {
            source: replace_range(source, position..position, &entry),
            notes: notes.trim().to_owned(),
        })
    }

    fn next(&self, source: &str, released: &Version, next: &Version) -> Result<String> {
        validate_stable(released)?;
        validate_stable(next)?;
        MarkdownDocument::parse(source)?;
        if next <= released {
            return Err(Error::invalid(
                "Next development version must exceed the release.",
            ));
        }
        Ok(source.to_owned())
    }

    fn allows_missing_file(&self) -> bool {
        true
    }
}

fn normalized_notes(notes: &str) -> Result<String> {
    Ok(LineEnding::Lf.normalize(notes)?.trim().to_owned())
}

fn validate_notes(notes: &str, captured: Option<&str>) -> Result<()> {
    if !has_meaningful_notes(notes)? {
        return Err(Error::invalid(
            "Release notes must contain meaningful content.",
        ));
    }
    if let Some(captured) = captured
        && normalized_notes(notes)? != normalized_notes(captured)?
    {
        return Err(Error::invalid("Release notes differ from captured notes."));
    }
    Ok(())
}

pub(crate) fn validate_stable(version: &Version) -> Result<()> {
    if !version.pre.is_empty() || !version.build.is_empty() {
        return Err(Error::invalid(
            "Only stable numeric versions are supported.",
        ));
    }
    Ok(())
}

/// Validate a real Gregorian date supplied by the invocation clock.
pub fn validate_date(date: &str) -> Result<()> {
    let bytes = date.as_bytes();
    let invalid = || Error::invalid("Expected a valid YYYY/MM/DD release date.");
    if bytes.len() != 10
        || bytes[4] != b'/'
        || bytes[7] != b'/'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| index != 4 && index != 7 && !byte.is_ascii_digit())
    {
        return Err(invalid());
    }
    let year: u32 = date[..4].parse().map_err(|_| invalid())?;
    let month: usize = date[5..7].parse().map_err(|_| invalid())?;
    let day: u32 = date[8..].parse().map_err(|_| invalid())?;
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if year == 0 || !(1..=12).contains(&month) || day == 0 || day > days[month - 1] {
        return Err(invalid());
    }
    Ok(())
}

#[derive(Debug)]
struct Reference {
    label: String,
    url: String,
    range: Range<usize>,
}

// Definitions are absent from pulldown-cmark's event stream. Only inspect the
// uncovered source, never definition-shaped text inside rendered Markdown.
fn references(document: &MarkdownDocument) -> Result<Vec<Reference>> {
    let mut references = Vec::new();
    let mut labels = BTreeSet::new();
    let mut offset = 0;
    for line in document.source().split_inclusive('\n') {
        let raw = line.trim_end_matches(['\r', '\n']);
        let indent = raw.bytes().take_while(|byte| *byte == b' ').count();
        let text = &raw[indent..];
        if indent <= 3
            && !document.block_at(offset + indent)
            && !document.opaque_at(offset + indent)
            && let Some(rest) = text.strip_prefix('[')
            && let Some((label, target)) = rest.split_once("]:")
        {
            let label = label
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            if label.is_empty() || !labels.insert(label.clone()) {
                return Err(Error::invalid(
                    "Duplicate or empty changelog link definition.",
                ));
            }
            let target = target.trim();
            let target = if target.starts_with('<') && target.ends_with('>') {
                &target[1..target.len() - 1]
            } else {
                target
            };
            if target.is_empty() || target.chars().any(char::is_whitespace) {
                return Err(Error::invalid("Unsupported changelog link definition."));
            }
            let after = &document.source()[offset + line.len()..];
            if after.lines().next().is_some_and(|next| {
                let next = next.trim_start();
                next.starts_with(['"', '\'']) || next.starts_with('(')
            }) {
                return Err(Error::invalid(
                    "Multiline changelog link definitions are unsupported.",
                ));
            }
            references.push(Reference {
                label,
                url: target.to_owned(),
                range: offset..offset + raw.len(),
            });
        }
        offset += line.len();
    }
    Ok(references)
}

fn notes_without_references(notes: &str) -> Result<String> {
    let document = MarkdownDocument::parse(notes)?;
    let mut notes = notes.to_owned();
    for reference in references(&document)?.into_iter().rev() {
        notes.replace_range(reference.range, "");
    }
    Ok(notes)
}

fn update_comparison(source: &str, version: &Version) -> Result<String> {
    let document = MarkdownDocument::parse(source)?;
    let references = references(&document)?;
    if references
        .iter()
        .any(|reference| reference.label == version.to_string())
    {
        return Err(Error::invalid(
            "Release comparison definition already exists.",
        ));
    }
    let Some(unreleased) = references
        .iter()
        .find(|reference| reference.label == "unreleased")
    else {
        return Ok(source.to_owned());
    };
    let comparison = comparison(&unreleased.url)?;
    if version <= &comparison.previous_version {
        return Err(Error::invalid(
            "Release must exceed the Unreleased comparison baseline.",
        ));
    }
    let Comparison {
        prefix,
        previous,
        tag_prefix,
        branch,
        ..
    } = comparison;
    let newline = document.line_ending().as_str();
    let updated = format!(
        "[Unreleased]: {prefix}/compare/{tag_prefix}{version}...{branch}{newline}[{version}]: {prefix}/compare/{previous}...{tag_prefix}{version}"
    );
    Ok(replace_range(source, unreleased.range.clone(), &updated))
}

struct Comparison<'a> {
    prefix: &'a str,
    previous: &'a str,
    tag_prefix: &'a str,
    previous_version: Version,
    branch: &'a str,
}

fn comparison(url: &str) -> Result<Comparison<'_>> {
    let (prefix, range) = url
        .rsplit_once("/compare/")
        .ok_or_else(|| Error::invalid("Unreleased reference must be a comparison URL."))?;
    if !(prefix.starts_with("https://") || prefix.starts_with("http://"))
        || prefix.contains(['<', '>'])
    {
        return Err(Error::invalid(
            "Unreleased comparison URL must use HTTP or HTTPS.",
        ));
    }
    let (previous, branch) = range
        .split_once("...")
        .ok_or_else(|| Error::invalid("Unreleased comparison must be VERSION...BRANCH."))?;
    if branch.is_empty()
        || branch.contains(['?', '#', '<', '>'])
        || branch.contains("...")
        || previous.contains('/')
    {
        return Err(Error::invalid("Unsupported Unreleased comparison range."));
    }
    let numeric = previous.char_indices().find_map(|(index, character)| {
        if !character.is_ascii_digit() {
            return None;
        }
        let version = Version::parse(&previous[index..]).ok()?;
        validate_stable(&version).ok()?;
        Some((index, version))
    });
    let (number_start, previous_version) = numeric.ok_or_else(|| {
        Error::invalid("Unreleased comparison requires a stable numeric baseline.")
    })?;
    Ok(Comparison {
        prefix,
        previous,
        tag_prefix: &previous[..number_start],
        previous_version,
        branch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(version: &str) -> Version {
        Version::parse(version).unwrap()
    }

    #[test]
    fn intridea_removes_all_real_prompts_and_restores_only_direct_categories() {
        let source = "# Changelog 🦀\n\n## 0.1.0 (Next)\n\n### Added\n\n- A useful feature.\n- Your contribution here.\n\n#### Details\n\nMore information.\n\n### Fixed\n\n- A repair.\n- Your contribution here.\n\n```\n- Your contribution here.\n```\n\n## 0.0.9 (2026/01/01)\n\n- History 雪.\n";
        let changelog = IntrideaChangelog::default();
        let edit = changelog
            .release(source, &v("0.1.0"), "2026/11/11", None)
            .unwrap();
        assert!(edit.source.contains("## 0.1.0 (2026/11/11)"));
        assert!(edit.notes.contains("```\n- Your contribution here.\n```"));
        assert_eq!(edit.notes.matches("- Your contribution here.").count(), 1);
        assert!(
            edit.source
                .ends_with("## 0.0.9 (2026/01/01)\n\n- History 雪.\n")
        );
        let next = changelog
            .next(&edit.source, &v("0.1.0"), &v("0.1.1"))
            .unwrap();
        let pending = MarkdownDocument::parse(&next).unwrap();
        let pending = pending.section("0.1.1 (Next)").unwrap();
        let body = &next[pending.body..pending.end];
        assert!(body.contains("### Added\n\n- Your contribution here."));
        assert!(body.contains("### Fixed\n\n- Your contribution here."));
        assert!(!body.contains("#### Details"));
        assert!(!body.contains("useful feature"));
        assert_eq!(
            changelog.next(&next, &v("0.1.0"), &v("0.1.1")).unwrap(),
            next
        );
        let rerun = changelog
            .release(&edit.source, &v("0.1.0"), "2026/12/01", None)
            .unwrap();
        assert_eq!(rerun, edit);
    }

    #[test]
    fn intridea_requires_prompt_and_meaningful_notes_without_mutation() {
        let changelog = IntrideaChangelog::default();
        for source in [
            "## 0.1.0 (Next)\n\n- Your contribution here.\n",
            "## 0.1.0 (Next)\n\n### Added\n\n<!-- comment -->\n\n- Your contribution here.\n",
            "## 0.1.0 (Next)\n\n- Feature without prompt.\n",
            "## 0.1.0 (Next)\n\n- Feature.\n\n> - Your contribution here.\n",
            "## 0.1.0 (Next)\n\n- A.\n- Your contribution here.\n\n## 0.1.0 (Next)\n\n- B.\n",
            "",
        ] {
            assert!(
                changelog
                    .release(source, &v("0.1.0"), "2026/11/11", None)
                    .is_err(),
                "{source}"
            );
        }
        assert!(
            changelog
                .release(
                    "## 0.1.0 (2026/11/11)\n\n<!-- empty -->",
                    &v("0.1.0"),
                    "2026/11/11",
                    None
                )
                .is_err()
        );
    }

    #[test]
    fn keep_preserves_crlf_unicode_history_and_comparisons() {
        let source = "# Changelog 🦀\r\n\r\n## [Unreleased]\r\n\r\n### Added\r\n\r\n- 雪 support.\r\n\r\n## [0.1.0] - 2026-01-01\r\n\r\n- Old.\r\n\r\n[Unreleased]: https://github.com/acme/crate/compare/v0.1.0...HEAD\r\n[0.1.0]: https://github.com/acme/crate/releases/tag/v0.1.0\r\n";
        let changelog = KeepAChangelog::default();
        let edit = changelog
            .release(source, &v("0.2.0"), "2026/11/11", None)
            .unwrap();
        assert_eq!(edit.notes, "### Added\r\n\r\n- 雪 support.");
        assert!(edit.source.contains("## [0.2.0] - 2026-11-11\r\n"));
        assert!(edit.source.contains("[Unreleased]: https://github.com/acme/crate/compare/v0.2.0...HEAD\r\n[0.2.0]: https://github.com/acme/crate/compare/v0.1.0...v0.2.0\r\n"));
        assert!(
            edit.source
                .ends_with("[0.1.0]: https://github.com/acme/crate/releases/tag/v0.1.0\r\n")
        );
        assert_eq!(LineEnding::detect(&edit.source).unwrap(), LineEnding::CrLf);
        let next = changelog
            .next(&edit.source, &v("0.2.0"), &v("0.2.1"))
            .unwrap();
        assert!(next.contains("## [Unreleased]\r\n\r\n## [0.2.0]"));
        assert_eq!(
            changelog
                .release(&next, &v("0.2.0"), "2026/12/01", Some(&edit.notes))
                .unwrap()
                .source,
            next
        );
        assert!(
            changelog
                .release(&next, &v("0.2.0"), "2026/11/11", Some("Conflicting notes"))
                .is_err()
        );
    }

    #[test]
    fn keep_templates_remove_empty_categories_and_restore_configured_categories() {
        let changelog = KeepAChangelog::new(
            ChangelogTemplate::new(
                Some("* Contribute here.".to_owned()),
                vec!["Added".to_owned(), "Fixed".to_owned()],
            )
            .unwrap(),
        );
        let source = "## [Unreleased]\n\n### Added\n\n- Feature.\n* Contribute here.\n\n### Fixed\n\n* Contribute here.\n";
        let edit = changelog
            .release(source, &v("1.0.0"), "2024/02/29", None)
            .unwrap();
        assert!(edit.notes.contains("### Added"));
        assert!(!edit.notes.contains("### Fixed"));
        assert!(!edit.notes.contains("Contribute"));
        let next = changelog
            .next(&edit.source, &v("1.0.0"), &v("1.0.1"))
            .unwrap();
        assert!(next.starts_with("## [Unreleased]\n\n### Added\n\n* Contribute here.\n\n### Fixed\n\n* Contribute here.\n\n"));
        assert!(ChangelogTemplate::new(Some("".to_owned()), vec![]).is_err());
        assert!(
            ChangelogTemplate::new(None, vec!["Added".to_owned(), "Added".to_owned()]).is_err()
        );
        assert!(IntrideaChangelog::new("bad\nprompt").is_err());
    }

    #[test]
    fn ambiguous_headings_definitions_and_next_positions_fail() {
        let changelog = KeepAChangelog::default();
        for source in [
            "## [Unreleased]\n\n- Feature.\n\n## [Unreleased]\n",
            "## [Unreleased]\n\n- Feature.\n\n[Unreleased]: https://example.com/compare/v1.0.0...HEAD\n[unreleased]: https://example.com/compare/v1.0.0...HEAD\n",
            "## [Unreleased]\n\n- Feature.\n\n[Unreleased]: https://example.com/not-a-comparison\n",
            "## [Unreleased]\n\n- Feature.\n\n[Unreleased]: https://example.com/compare/v1.0.0...HEAD \"title\"\n",
            "## [Unreleased]\n\n- Feature.\n\n[2.0.0]: https://example.com/release\n",
            "## [2.0.0] - yesterday\n\n- Feature.\n",
            "## [2.0.0] - 2026-01-01\n\n- A.\n\n## [2.0.0] - 2026-02-01\n\n- B.\n",
        ] {
            assert!(
                changelog
                    .release(source, &v("2.0.0"), "2026/11/11", None)
                    .is_err(),
                "{source}"
            );
        }
        let source = "## [1.0.0] - 2026-01-01\n\n- Old.\n\n## [Unreleased]\n";
        assert!(changelog.next(source, &v("1.0.0"), &v("1.0.1")).is_err());
        let template = ChangelogTemplate::new(None, vec!["Added".to_owned()]).unwrap();
        assert!(
            KeepAChangelog::new(template)
                .next(
                    "###### [1.0.0] - 2026-01-01\n\n- Note.\n",
                    &v("1.0.0"),
                    &v("1.0.1")
                )
                .is_err()
        );
    }

    #[test]
    fn generated_notes_create_reconcile_and_preserve_history() {
        let changelog = GeneratedChangelog;
        let edit = changelog
            .release(
                "",
                &v("1.0.0"),
                "2026/11/11",
                Some("### Features\n\n- New 🦀."),
            )
            .unwrap();
        assert!(
            edit.source
                .starts_with("# Changelog\n\n## 1.0.0 (2026/11/11)\n\n")
        );
        assert_eq!(
            changelog
                .release(&edit.source, &v("1.0.0"), "2026/12/01", Some(&edit.notes))
                .unwrap()
                .source,
            edit.source
        );
        assert_eq!(
            changelog
                .next(&edit.source, &v("1.0.0"), &v("1.0.1"))
                .unwrap(),
            edit.source
        );
        let old = "# Changelog\r\n\r\nPreamble 雪.\r\n\r\n```\r\n## Example\r\n```\r\n\r\n## 0.1.0 (2026/01/01)\r\n\r\n- Old.\r\n";
        let new = changelog
            .release(old, &v("1.0.0"), "2026/11/11", Some("- New.\n- More."))
            .unwrap();
        assert!(new.source.starts_with(
            "# Changelog\r\n\r\nPreamble 雪.\r\n\r\n```\r\n## Example\r\n```\r\n\r\n## 1.0.0"
        ));
        assert!(
            new.source
                .ends_with("## 0.1.0 (2026/01/01)\r\n\r\n- Old.\r\n")
        );
        assert_eq!(LineEnding::detect(&new.source).unwrap(), LineEnding::CrLf);
        assert!(
            changelog
                .release(&new.source, &v("1.0.0"), "2026/11/11", Some("- Different."))
                .is_err()
        );
        let duplicated = format!("{}{}", edit.source, "## 1.0.0 (2026/11/11)\n\n- New.\n");
        assert!(
            changelog
                .release(&duplicated, &v("1.0.0"), "2026/11/11", Some(&edit.notes))
                .is_err()
        );
    }

    #[test]
    fn generated_file_validation_precedes_creation_or_writing() {
        let directory = tempfile::tempdir_in(".").unwrap();
        let path = directory.path().join("CHANGELOG.md");
        let changelog = GeneratedChangelog;
        for notes in [
            None,
            Some(""),
            Some("## Heading\n\n<!-- empty -->\n\n---\n"),
            Some("- Your contribution here."),
        ] {
            assert!(
                changelog
                    .release_file(&path, &v("1.0.0"), "2026/11/11", notes)
                    .is_err()
            );
            assert!(!path.exists());
        }
        let edit = changelog
            .release_file(&path, &v("1.0.0"), "2026/11/11", Some("- Feature."))
            .unwrap();
        assert!(
            changelog
                .release_file(&path, &v("1.0.0"), "2026/11/11", Some("- Conflict."))
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edit.source);
        assert!(
            KeepAChangelog::default()
                .release_file(
                    &directory.path().join("MISSING.md"),
                    &v("1.0.0"),
                    "2026/11/11",
                    None
                )
                .is_err()
        );
    }

    #[test]
    fn invalid_dates_versions_and_line_endings_fail() {
        for date in [
            "2026-11-11",
            "2026/02/29",
            "1900/02/29",
            "0000/01/01",
            "2026/13/01",
            "2026/00/01",
            "2026/01/00",
            "2026/04/31",
            "雪/11/11",
        ] {
            assert!(validate_date(date).is_err(), "{date}");
        }
        assert!(validate_date("2000/02/29").is_ok());
        assert!(
            GeneratedChangelog
                .release("", &v("1.0.0-rc.1"), "2026/11/11", Some("- Note."))
                .is_err()
        );
        assert!(
            GeneratedChangelog
                .release(
                    "# Changelog\r\n\n",
                    &v("1.0.0"),
                    "2026/11/11",
                    Some("- Note.")
                )
                .is_err()
        );
        assert!(
            GeneratedChangelog
                .release("", &v("1.0.0"), "2026/11/11", Some("- Bad.\r\n- Mixed.\n"))
                .is_err()
        );
        assert!(
            KeepAChangelog::default()
                .next(
                    "## [1.0.0] - 2026-11-11\n\n- Note.\n",
                    &v("1.0.0"),
                    &v("1.0.0")
                )
                .is_err()
        );
    }

    #[test]
    fn custom_tag_prefixes_reference_examples_and_next_baselines_are_preserved() {
        let source = "# Changelog\n\n# [Unreleased]\n\n- Feature.\n\n```\n[Unreleased]: not-a-comparison\n```\n\n[Unreleased]: https://example.com/team/crate/compare/release-1.0.0...development/main\n";
        let changelog = KeepAChangelog::default();
        let edit = changelog
            .release(source, &v("1.1.0"), "2026/11/11", None)
            .unwrap();
        assert!(edit.source.contains("# [1.1.0] - 2026-11-11"));
        assert!(
            edit.source
                .contains("```\n[Unreleased]: not-a-comparison\n```")
        );
        assert!(edit.source.contains("[Unreleased]: https://example.com/team/crate/compare/release-1.1.0...development/main\n[1.1.0]: https://example.com/team/crate/compare/release-1.0.0...release-1.1.0"));
        assert!(
            changelog
                .next(&edit.source, &v("1.1.0"), &v("1.1.1"))
                .unwrap()
                .contains("# [Unreleased]\n\n# [1.1.0]")
        );
        let stale = edit.source.replace(
            "release-1.1.0...development/main",
            "release-1.0.0...development/main",
        );
        assert!(changelog.next(&stale, &v("1.1.0"), &v("1.1.1")).is_err());
        assert!(
            changelog
                .release(
                    "# [1.1.0] - 2026/11/11\n\n- Note.",
                    &v("1.1.0"),
                    "2026/11/11",
                    None
                )
                .is_err()
        );
    }

    #[test]
    fn captured_notes_do_not_include_final_comparison_definitions() {
        let source = "## [Unreleased]\n\n- First release.\n\n[Unreleased]: https://example.com/compare/0.0.1...HEAD\n";
        let changelog = KeepAChangelog::default();
        let edit = changelog
            .release(source, &v("1.0.0"), "2026/11/11", None)
            .unwrap();
        assert_eq!(edit.notes, "- First release.");
        assert!(
            edit.source
                .contains("[1.0.0]: https://example.com/compare/0.0.1...1.0.0")
        );
        assert_eq!(
            changelog
                .release(
                    &edit.source,
                    &v("1.0.0"),
                    "2026/12/01",
                    Some("- First release.")
                )
                .unwrap(),
            edit
        );
    }
}
