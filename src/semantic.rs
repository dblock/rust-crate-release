//! Conventional Commit analysis and immutable, source-bound release decisions.

use std::collections::BTreeSet;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::changelog::validate_stable;
use crate::markdown::{LineEnding, has_meaningful_notes};
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseType {
    Patch,
    Minor,
    Major,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SemanticCommit {
    pub sha: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SemanticTag {
    pub tag: String,
    pub version: Version,
    pub commit: String,
}

/// History is supplied by the Git adapter, already restricted to the captured
/// source and the commits after the highest reachable stable release tag.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SemanticHistoryData {
    pub last_release: Option<SemanticTag>,
    pub commits: Vec<SemanticCommit>,
}

pub trait SemanticHistory: Send + Sync {
    fn read(&self, source_sha: &str) -> Result<SemanticHistoryData>;
}

pub trait CommitAnalyzer: Send + Sync {
    fn analyze(&self, commits: &[SemanticCommit]) -> Result<Option<ReleaseType>>;
}

#[derive(Clone, Copy, Debug)]
pub struct SemanticSelection<'a> {
    pub last_release: Option<&'a Version>,
    pub release_type: Option<ReleaseType>,
    pub explicit_version: Option<&'a Version>,
}

pub trait SemanticVersionPolicy: Send + Sync {
    fn select(&self, selection: SemanticSelection<'_>) -> Result<Option<Version>>;
}

pub trait SemanticNotes: Send + Sync {
    fn generate(&self, commits: &[SemanticCommit]) -> Result<String>;
}

/// Optional rule selectors use logical AND; matching rules replace default behavior.
/// `release: None` suppresses a commit. `*` and `?` are the only wildcards.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReleaseRule {
    pub commit_type: Option<String>,
    pub scope: Option<String>,
    pub subject: Option<String>,
    pub breaking: Option<bool>,
    pub revert: Option<bool>,
    pub release: Option<ReleaseType>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedCommit {
    pub commit: SemanticCommit,
    pub commit_type: Option<String>,
    pub scope: Option<String>,
    pub subject: String,
    pub breaking: bool,
    pub breaking_notes: Vec<String>,
    pub revert: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ConventionalCommits {
    rules: Vec<ReleaseRule>,
}

impl ConventionalCommits {
    pub fn new(rules: Vec<ReleaseRule>) -> Self {
        Self { rules }
    }

    pub fn rules(&self) -> &[ReleaseRule] {
        &self.rules
    }

    /// Parse full messages, then cancel original/revert pairs in reverse order.
    /// A revert of a revert therefore restores the original change.
    pub fn parsed(&self, commits: &[SemanticCommit]) -> Result<Vec<ParsedCommit>> {
        validate_commits(commits)?;
        let parsed: Vec<_> = commits
            .iter()
            .filter(|commit| !commit.message.trim().is_empty())
            .map(parse_commit)
            .collect();
        let mut cancelled = vec![false; parsed.len()];
        for index in (0..parsed.len()).rev() {
            if cancelled[index] {
                continue;
            }
            let Some(reverted) = &parsed[index].revert else {
                continue;
            };
            let matches: Vec<_> = parsed
                .iter()
                .enumerate()
                .filter(|(candidate, commit)| {
                    *candidate != index
                        && !cancelled[*candidate]
                        && commit.commit.sha.starts_with(reverted)
                })
                .map(|(index, _)| index)
                .collect();
            if matches.len() > 1 {
                return Err(Error::invalid("Ambiguous abbreviated reverted commit SHA."));
            }
            if let Some(original) = matches.first() {
                cancelled[*original] = true;
                cancelled[index] = true;
            }
        }
        Ok(parsed
            .into_iter()
            .enumerate()
            .filter_map(|(index, commit)| (!cancelled[index]).then_some(commit))
            .collect())
    }
}

impl CommitAnalyzer for ConventionalCommits {
    fn analyze(&self, commits: &[SemanticCommit]) -> Result<Option<ReleaseType>> {
        let mut selected = None;
        for commit in self.parsed(commits)? {
            let matching: Vec<_> = self
                .rules
                .iter()
                .filter(|rule| rule.matches(&commit))
                .collect();
            let release = if matching.is_empty() {
                if commit.breaking {
                    Some(ReleaseType::Major)
                } else {
                    match commit.commit_type.as_deref() {
                        Some("feat") => Some(ReleaseType::Minor),
                        Some("fix" | "perf") => Some(ReleaseType::Patch),
                        _ if commit.revert.is_some() => Some(ReleaseType::Patch),
                        _ => None,
                    }
                }
            } else {
                matching.iter().filter_map(|rule| rule.release).max()
            };
            selected = selected.max(release);
        }
        Ok(selected)
    }
}

impl ReleaseRule {
    fn matches(&self, commit: &ParsedCommit) -> bool {
        [
            (&self.commit_type, commit.commit_type.as_deref()),
            (&self.scope, commit.scope.as_deref()),
            (&self.subject, Some(commit.subject.as_str())),
        ]
        .iter()
        .all(|(pattern, value)| {
            pattern
                .as_deref()
                .is_none_or(|pattern| value.is_some_and(|value| glob_matches(pattern, value)))
        }) && self
            .breaking
            .is_none_or(|breaking| breaking == commit.breaking)
            && self
                .revert
                .is_none_or(|revert| revert == commit.revert.is_some())
    }
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let value: Vec<_> = value.chars().collect();
    let mut matched = vec![false; value.len() + 1];
    matched[0] = true;
    for character in pattern.chars() {
        if character == '*' {
            for index in 1..matched.len() {
                let previous = matched[index - 1];
                matched[index] |= previous;
            }
        } else {
            for index in (1..matched.len()).rev() {
                matched[index] =
                    matched[index - 1] && (character == '?' || character == value[index - 1]);
            }
            matched[0] = false;
        }
    }
    matched[value.len()]
}

fn parse_commit(commit: &SemanticCommit) -> ParsedCommit {
    let lines: Vec<_> = commit.message.lines().collect();
    let first = lines.first().copied().unwrap_or("");
    let header = parse_header(first);
    let subject = header.as_ref().map_or(first, |header| header.subject);
    let mut breaking_notes = Vec::new();
    let mut footer = false;
    let mut index = 1;
    while index < lines.len() {
        let Some(note) = breaking_footer(lines[index]) else {
            index += 1;
            continue;
        };
        footer = true;
        let mut notes = vec![note];
        index += 1;
        while index < lines.len() && !looks_like_trailer(lines[index]) {
            notes.push(lines[index]);
            index += 1;
        }
        let notes = notes.join("\n").trim().to_owned();
        if !notes.is_empty() {
            breaking_notes.push(notes);
        }
    }
    let breaking = header.as_ref().is_some_and(|header| header.breaking) || footer;
    if breaking && breaking_notes.is_empty() {
        breaking_notes.push(subject.to_owned());
    }
    let revert = lines.iter().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        let sha = lower
            .strip_prefix("this reverts commit ")?
            .strip_suffix('.')?;
        ((7..=64).contains(&sha.len()) && sha.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| sha.to_owned())
    });
    ParsedCommit {
        commit: commit.clone(),
        commit_type: header.as_ref().map(|header| header.commit_type.to_owned()),
        scope: header
            .as_ref()
            .and_then(|header| header.scope.map(str::to_owned)),
        subject: subject.to_owned(),
        breaking,
        breaking_notes,
        revert,
    }
}

struct Header<'a> {
    commit_type: &'a str,
    scope: Option<&'a str>,
    subject: &'a str,
    breaking: bool,
}

fn parse_header(line: &str) -> Option<Header<'_>> {
    let (prefix, subject) = line.split_once(": ")?;
    if subject.trim().is_empty() {
        return None;
    }
    let breaking = prefix.ends_with('!');
    let prefix = if breaking {
        &prefix[..prefix.len() - 1]
    } else {
        prefix
    };
    let (commit_type, scope) = if let Some((kind, scope)) = prefix.split_once('(') {
        let scope = scope.strip_suffix(')')?;
        if scope.is_empty() || scope.contains(['(', ')']) {
            return None;
        }
        (kind, Some(scope))
    } else {
        (prefix, None)
    };
    if commit_type.is_empty()
        || !commit_type
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some(Header {
        commit_type,
        scope,
        subject,
        breaking,
    })
}

fn breaking_footer(line: &str) -> Option<&str> {
    line.strip_prefix("BREAKING CHANGE:")
        .or_else(|| line.strip_prefix("BREAKING-CHANGE:"))
        .map(|text| text.trim_start_matches([' ', '\t']))
}

fn looks_like_trailer(line: &str) -> bool {
    if breaking_footer(line).is_some() {
        return true;
    }
    let key = line
        .split_once(": ")
        .or_else(|| line.split_once(" #"))
        .map(|(key, _)| key);
    key.is_some_and(|key| {
        key.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
            && key
                .bytes()
                .all(|byte| byte.is_ascii_alphabetic() || byte == b'-')
    })
}

#[derive(Clone, Debug)]
pub struct SemanticVersionSelector {
    first_version: Version,
}

impl Default for SemanticVersionSelector {
    fn default() -> Self {
        Self {
            first_version: Version::new(1, 0, 0),
        }
    }
}

impl SemanticVersionSelector {
    pub fn new(first_version: Version) -> Result<Self> {
        validate_stable(&first_version)?;
        Ok(Self { first_version })
    }
}

impl SemanticVersionPolicy for SemanticVersionSelector {
    fn select(&self, selection: SemanticSelection<'_>) -> Result<Option<Version>> {
        if let Some(previous) = selection.last_release {
            validate_stable(previous)?;
        }
        let selected = if let Some(explicit) = selection.explicit_version {
            validate_stable(explicit)?;
            Some(explicit.clone())
        } else if let Some(release_type) = selection.release_type {
            match selection.last_release {
                Some(previous) => Some(increment(previous, release_type)?),
                None => Some(self.first_version.clone()),
            }
        } else {
            None
        };
        if let (Some(selected), Some(previous)) = (&selected, selection.last_release)
            && selected <= previous
        {
            return Err(Error::invalid(
                "Selected semantic version must exceed the previous release.",
            ));
        }
        Ok(selected)
    }
}

fn increment(previous: &Version, release_type: ReleaseType) -> Result<Version> {
    let overflow = || Error::invalid("Semantic version component overflow.");
    Ok(match release_type {
        ReleaseType::Major => {
            Version::new(previous.major.checked_add(1).ok_or_else(overflow)?, 0, 0)
        }
        ReleaseType::Minor => Version::new(
            previous.major,
            previous.minor.checked_add(1).ok_or_else(overflow)?,
            0,
        ),
        ReleaseType::Patch => Version::new(
            previous.major,
            previous.minor,
            previous.patch.checked_add(1).ok_or_else(overflow)?,
        ),
    })
}

#[derive(Clone, Debug, Default)]
pub struct GeneratedReleaseNotes;

impl SemanticNotes for GeneratedReleaseNotes {
    fn generate(&self, commits: &[SemanticCommit]) -> Result<String> {
        let parsed = ConventionalCommits::default().parsed(commits)?;
        let mut sections = Vec::new();
        let mut seen_breaking = BTreeSet::new();
        let breaking: Vec<_> = parsed
            .iter()
            .flat_map(|commit| &commit.breaking_notes)
            .filter(|note| seen_breaking.insert((*note).clone()))
            .map(|note| format!("- {}", escape_markdown(note).replace('\n', "\n  ")))
            .collect();
        if !breaking.is_empty() {
            sections.push(format!("### Breaking Changes\n\n{}", breaking.join("\n")));
        }
        for (kind, title) in [
            ("feat", "Features"),
            ("fix", "Bug Fixes"),
            ("perf", "Performance Improvements"),
            ("other", "Other Changes"),
        ] {
            let mut seen = BTreeSet::new();
            let mut notes = Vec::new();
            for commit in &parsed {
                let matches = if kind == "other" {
                    !matches!(commit.commit_type.as_deref(), Some("feat" | "fix" | "perf"))
                } else {
                    commit.commit_type.as_deref() == Some(kind)
                };
                if !matches || !seen.insert((commit.scope.clone(), commit.subject.clone())) {
                    continue;
                }
                let scope = commit.scope.as_deref().map_or_else(String::new, |scope| {
                    format!("**{}:** ", escape_markdown(scope))
                });
                notes.push(format!(
                    "- {scope}{} ({})",
                    escape_markdown(&commit.subject),
                    &commit.commit.sha[..7]
                ));
            }
            if !notes.is_empty() {
                sections.push(format!("### {title}\n\n{}", notes.join("\n")));
            }
        }
        Ok(if sections.is_empty() {
            "Explicit version release.".to_owned()
        } else {
            sections.join("\n\n")
        })
    }
}

fn escape_markdown(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        if "\\`*_[]<>#!|".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// Serializable invocation-local data. Construct a `SemanticDecision` to
/// validate it before using it as release authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticDecisionData {
    pub source_sha: String,
    pub manifest_version: Version,
    pub input_override: Option<Version>,
    pub last_release: Option<SemanticTag>,
    pub commits: Vec<SemanticCommit>,
    pub release_type: Option<ReleaseType>,
    pub version: Option<Version>,
    pub notes: Option<String>,
}

/// Immutable owned analysis: deserialization validates the same invariants as
/// construction, and accessors never expose mutable decision data.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "SemanticDecisionData", into = "SemanticDecisionData")]
pub struct SemanticDecision {
    data: SemanticDecisionData,
}

impl SemanticDecision {
    pub fn new(data: SemanticDecisionData) -> Result<Self> {
        validate_sha(&data.source_sha)?;
        validate_stable(&data.manifest_version)?;
        validate_history(&SemanticHistoryData {
            last_release: data.last_release.clone(),
            commits: data.commits.clone(),
        })?;
        if let Some(explicit) = &data.input_override {
            validate_stable(explicit)?;
        }
        match &data.version {
            None => {
                if data.release_type.is_some()
                    || data.notes.is_some()
                    || data.input_override.is_some()
                {
                    return Err(Error::invalid("Invalid non-release semantic decision."));
                }
            }
            Some(version) => {
                validate_stable(version)?;
                if data.release_type.is_none() && data.input_override.is_none() {
                    return Err(Error::invalid(
                        "Semantic release requires eligible commits or an override.",
                    ));
                }
                if data
                    .input_override
                    .as_ref()
                    .is_some_and(|explicit| explicit != version)
                {
                    return Err(Error::invalid(
                        "Selected version differs from the explicit override.",
                    ));
                }
                if data
                    .last_release
                    .as_ref()
                    .is_some_and(|previous| version <= &previous.version)
                {
                    return Err(Error::invalid(
                        "Semantic decision does not exceed the previous release.",
                    ));
                }
                let notes = data
                    .notes
                    .as_deref()
                    .ok_or_else(|| Error::invalid("Missing semantic release notes."))?;
                if !has_meaningful_notes(notes)? {
                    return Err(Error::invalid(
                        "Semantic release notes must contain meaningful content.",
                    ));
                }
            }
        }
        Ok(Self { data })
    }

    pub fn source_sha(&self) -> &str {
        &self.data.source_sha
    }

    pub fn manifest_version(&self) -> &Version {
        &self.data.manifest_version
    }

    pub fn input_override(&self) -> Option<&Version> {
        self.data.input_override.as_ref()
    }

    pub fn last_release(&self) -> Option<&SemanticTag> {
        self.data.last_release.as_ref()
    }

    pub fn commits(&self) -> &[SemanticCommit] {
        &self.data.commits
    }

    pub fn release_type(&self) -> Option<ReleaseType> {
        self.data.release_type
    }

    pub fn version(&self) -> Option<&Version> {
        self.data.version.as_ref()
    }

    pub fn notes(&self) -> Option<&str> {
        self.data.notes.as_deref()
    }

    pub fn releasable(&self) -> bool {
        self.data.version.is_some()
    }

    pub fn require_release(&self) -> Result<(&Version, &str)> {
        self.version().zip(self.notes()).ok_or_else(|| {
            Error::invalid("No eligible semantic changes and no explicit version override.")
        })
    }

    /// Check original invocation inputs before any preparation edits.
    pub fn check(
        &self,
        source_sha: &str,
        manifest_version: &Version,
        selected: Option<&Version>,
        input_override: Option<&Version>,
    ) -> Result<()> {
        if source_sha != self.source_sha() {
            return Err(Error::invalid(
                "Semantic decision belongs to another source commit.",
            ));
        }
        if manifest_version != self.manifest_version() {
            return Err(Error::invalid(
                "Manifest input changed after semantic analysis.",
            ));
        }
        if selected != self.version() {
            return Err(Error::invalid(
                "Release version differs from semantic decision.",
            ));
        }
        if input_override != self.input_override() {
            return Err(Error::invalid(
                "Explicit version changed after semantic analysis.",
            ));
        }
        Ok(())
    }

    /// Check prepared output against captured identity and notes, not a new
    /// analysis of the release commit or of an advanced development manifest.
    pub fn validate_prepared(
        &self,
        source_sha: &str,
        version: &Version,
        notes: &str,
    ) -> Result<()> {
        let (captured_version, captured_notes) = self.require_release()?;
        if source_sha != self.source_sha() || version != captured_version {
            return Err(Error::invalid(
                "Prepared release identity differs from semantic decision.",
            ));
        }
        if LineEnding::Lf.normalize(notes)?.trim()
            != LineEnding::Lf.normalize(captured_notes)?.trim()
        {
            return Err(Error::invalid(
                "Prepared release notes differ from captured semantic notes.",
            ));
        }
        Ok(())
    }

    pub fn into_data(self) -> SemanticDecisionData {
        self.data
    }
}

impl TryFrom<SemanticDecisionData> for SemanticDecision {
    type Error = Error;

    fn try_from(data: SemanticDecisionData) -> Result<Self> {
        Self::new(data)
    }
}

impl From<SemanticDecision> for SemanticDecisionData {
    fn from(decision: SemanticDecision) -> Self {
        decision.into_data()
    }
}

pub struct SemanticRelease {
    analyzer: Box<dyn CommitAnalyzer>,
    selector: Box<dyn SemanticVersionPolicy>,
    notes: Box<dyn SemanticNotes>,
}

impl Default for SemanticRelease {
    fn default() -> Self {
        Self::new(
            Box::new(ConventionalCommits::default()),
            Box::new(SemanticVersionSelector::default()),
            Box::new(GeneratedReleaseNotes),
        )
    }
}

impl SemanticRelease {
    pub fn new(
        analyzer: Box<dyn CommitAnalyzer>,
        selector: Box<dyn SemanticVersionPolicy>,
        notes: Box<dyn SemanticNotes>,
    ) -> Self {
        Self {
            analyzer,
            selector,
            notes,
        }
    }

    pub fn analyze(
        &self,
        source_sha: &str,
        manifest_version: &Version,
        input_override: Option<&Version>,
        history: &SemanticHistoryData,
    ) -> Result<SemanticDecision> {
        validate_sha(source_sha)?;
        validate_stable(manifest_version)?;
        validate_history(history)?;
        let release_type = self.analyzer.analyze(&history.commits)?;
        let version = self.selector.select(SemanticSelection {
            last_release: history.last_release.as_ref().map(|tag| &tag.version),
            release_type,
            explicit_version: input_override,
        })?;
        let notes = if version.is_some() {
            Some(self.notes.generate(&history.commits)?)
        } else {
            None
        };
        SemanticDecision::new(SemanticDecisionData {
            source_sha: source_sha.to_owned(),
            manifest_version: manifest_version.clone(),
            input_override: input_override.cloned(),
            last_release: history.last_release.clone(),
            commits: history.commits.clone(),
            release_type,
            version,
            notes,
        })
    }
}

fn validate_sha(sha: &str) -> Result<()> {
    if !matches!(sha.len(), 40 | 64)
        || !sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::invalid(
            "Expected a complete lowercase Git commit SHA.",
        ));
    }
    Ok(())
}

fn validate_commits(commits: &[SemanticCommit]) -> Result<()> {
    let mut unique = BTreeSet::new();
    for commit in commits {
        validate_sha(&commit.sha)?;
        if !unique.insert(commit.sha.as_str()) {
            return Err(Error::invalid("Duplicate commit in semantic history."));
        }
    }
    Ok(())
}

fn validate_history(history: &SemanticHistoryData) -> Result<()> {
    validate_commits(&history.commits)?;
    if let Some(previous) = &history.last_release {
        validate_stable(&previous.version)?;
        validate_sha(&previous.commit)?;
        if previous.tag != format!("v{}", previous.version) {
            return Err(Error::invalid(
                "Semantic baseline must be a numeric vX.Y.Z release tag.",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(number: u64, message: &str) -> SemanticCommit {
        SemanticCommit {
            sha: format!("{number:040x}"),
            message: message.to_owned(),
        }
    }

    fn v(version: &str) -> Version {
        Version::parse(version).unwrap()
    }

    fn history(previous: Option<&str>, commits: Vec<SemanticCommit>) -> SemanticHistoryData {
        SemanticHistoryData {
            last_release: previous.map(|version| SemanticTag {
                tag: format!("v{version}"),
                version: v(version),
                commit: "a".repeat(40),
            }),
            commits,
        }
    }

    #[test]
    fn conventional_commits_classify_scopes_bodies_and_breaking_trailers() {
        let analyzer = ConventionalCommits::default();
        for (message, release) in [
            ("feat(api): Add widgets", Some(ReleaseType::Minor)),
            (
                "fix: Repair widgets\n\nAn ordinary body.",
                Some(ReleaseType::Patch),
            ),
            ("perf(core): Reduce allocations", Some(ReleaseType::Patch)),
            ("refactor!: Remove API", Some(ReleaseType::Major)),
            ("feat(core)!: Replace API", Some(ReleaseType::Major)),
            (
                "chore: Update\n\nBREAKING CHANGE: Remove the old API\nand its companion.\n\nRefs: #1",
                Some(ReleaseType::Major),
            ),
            (
                "fix: Update\r\n\r\nBREAKING-CHANGE: Remove old behavior",
                Some(ReleaseType::Major),
            ),
            ("docs: Expand examples", None),
            ("not a conventional commit", None),
            ("feat(no: Missing delimiter", None),
            ("feat: ", None),
            ("", None),
        ] {
            assert_eq!(
                analyzer.analyze(&[commit(1, message)]).unwrap(),
                release,
                "{message:?}"
            );
        }
        let parsed = analyzer.parsed(&[commit(1, "feat(api)!: Replace it\n\nBREAKING CHANGE: First line\nSecond line\n\nBREAKING-CHANGE: Another break\nSigned-off-by: Author")]).unwrap();
        assert_eq!(parsed[0].scope.as_deref(), Some("api"));
        assert_eq!(
            parsed[0].breaking_notes,
            ["First line\nSecond line", "Another break"]
        );
        assert_eq!(
            analyzer
                .analyze(&[
                    commit(1, "fix: Repair"),
                    commit(2, "feat: Add"),
                    commit(3, "chore!: Remove")
                ])
                .unwrap(),
            Some(ReleaseType::Major)
        );
    }

    #[test]
    fn rules_match_literal_unicode_globs_and_replace_defaults() {
        let analyzer = ConventionalCommits::new(vec![
            ReleaseRule {
                commit_type: Some("docs".to_owned()),
                scope: Some("pub*".to_owned()),
                subject: Some("Document ?🦀[API]".to_owned()),
                release: Some(ReleaseType::Patch),
                ..ReleaseRule::default()
            },
            ReleaseRule {
                scope: Some("internal".to_owned()),
                breaking: Some(true),
                release: None,
                ..ReleaseRule::default()
            },
        ]);
        assert_eq!(
            analyzer
                .analyze(&[commit(1, "docs(public): Document a🦀[API]")])
                .unwrap(),
            Some(ReleaseType::Patch)
        );
        assert_eq!(
            analyzer
                .analyze(&[commit(1, "docs(public): Document a🦀A")])
                .unwrap(),
            None
        );
        assert_eq!(
            analyzer
                .analyze(&[commit(1, "feat(internal)!: Remove private API")])
                .unwrap(),
            None
        );
        assert_eq!(
            analyzer
                .analyze(&[commit(1, "feat(public): Add public API")])
                .unwrap(),
            Some(ReleaseType::Minor)
        );
        let multiple = ConventionalCommits::new(vec![
            ReleaseRule {
                release: Some(ReleaseType::Minor),
                ..ReleaseRule::default()
            },
            ReleaseRule {
                release: Some(ReleaseType::Major),
                ..ReleaseRule::default()
            },
        ]);
        assert_eq!(
            multiple.analyze(&[commit(1, "docs: Expand")]).unwrap(),
            Some(ReleaseType::Major)
        );
    }

    #[test]
    fn reverts_cancel_restore_and_reject_ambiguous_abbreviations() {
        let original = commit(1, "feat: Add API");
        let reverted = commit(
            2,
            &format!(
                "revert: Revert API\n\nThis reverts commit {}.",
                original.sha
            ),
        );
        let restored = commit(
            3,
            &format!(
                "revert: Restore API\n\nThis reverts commit {}.",
                reverted.sha
            ),
        );
        let analyzer = ConventionalCommits::default();
        assert_eq!(
            analyzer
                .analyze(&[original.clone(), reverted.clone()])
                .unwrap(),
            None
        );
        assert_eq!(
            analyzer
                .analyze(&[original.clone(), reverted.clone(), restored])
                .unwrap(),
            Some(ReleaseType::Minor)
        );
        assert_eq!(
            analyzer.analyze(&[reverted]).unwrap(),
            Some(ReleaseType::Patch)
        );
        let ambiguous = commit(3, "revert: Revert\n\nThis reverts commit 0000000.");
        assert!(
            analyzer
                .parsed(&[original, commit(2, "fix: Repair"), ambiguous])
                .is_err()
        );
    }

    #[test]
    fn version_policy_first_breaking_override_and_noneligible_cases() {
        let selector = SemanticVersionSelector::default();
        let select = |previous: Option<&str>, release_type, explicit: Option<&str>| {
            let previous = previous.map(v);
            let explicit = explicit.map(v);
            selector.select(SemanticSelection {
                last_release: previous.as_ref(),
                release_type,
                explicit_version: explicit.as_ref(),
            })
        };
        assert_eq!(
            select(None, Some(ReleaseType::Patch), None).unwrap(),
            Some(v("1.0.0"))
        );
        assert_eq!(
            select(Some("0.3.8"), Some(ReleaseType::Major), None).unwrap(),
            Some(v("1.0.0"))
        );
        assert_eq!(
            select(Some("2.3.8"), Some(ReleaseType::Minor), None).unwrap(),
            Some(v("2.4.0"))
        );
        assert_eq!(
            select(Some("2.3.8"), Some(ReleaseType::Patch), None).unwrap(),
            Some(v("2.3.9"))
        );
        assert_eq!(select(Some("2.3.8"), None, None).unwrap(), None);
        assert_eq!(
            select(Some("2.3.8"), None, Some("2.4.0")).unwrap(),
            Some(v("2.4.0"))
        );
        assert_eq!(
            select(Some("2.3.8"), Some(ReleaseType::Major), Some("2.3.9")).unwrap(),
            Some(v("2.3.9"))
        );
        for explicit in ["2.3.8", "2.3.7", "3.0.0-rc.1", "3.0.0+build"] {
            assert!(select(Some("2.3.8"), Some(ReleaseType::Patch), Some(explicit)).is_err());
        }
        assert!(
            select(
                Some("18446744073709551615.0.0"),
                Some(ReleaseType::Major),
                None
            )
            .is_err()
        );
        let first = SemanticVersionSelector::new(v("0.1.0")).unwrap();
        assert_eq!(
            first
                .select(SemanticSelection {
                    last_release: None,
                    release_type: Some(ReleaseType::Minor),
                    explicit_version: None
                })
                .unwrap(),
            Some(v("0.1.0"))
        );
        assert!(SemanticVersionSelector::new(v("1.0.0-rc.1")).is_err());
    }

    #[test]
    fn generated_notes_are_grouped_escaped_deduplicated_and_multiline() {
        let commits = vec![
            commit(1, "fix: Repair [link]"),
            commit(
                2,
                "feat(api): Add *widgets*\n\nBREAKING CHANGE: Remove <old>\nand `legacy`",
            ),
            commit(3, "feat(api): Add *widgets*"),
            commit(4, "perf: Speed up"),
            commit(5, "docs: Document API"),
        ];
        let notes = GeneratedReleaseNotes.generate(&commits).unwrap();
        assert!(
            notes.starts_with("### Breaking Changes\n\n- Remove \\<old\\>\n  and \\`legacy\\`")
        );
        assert!(notes.contains("### Features\n\n- **api:** Add \\*widgets\\*"));
        assert_eq!(notes.matches("Add \\*widgets\\*").count(), 1);
        assert!(notes.find("### Features").unwrap() < notes.find("### Bug Fixes").unwrap());
        assert!(notes.contains("Repair \\[link\\]"));
        assert!(notes.contains("### Performance Improvements"));
        assert!(notes.contains("### Other Changes"));
        assert_eq!(
            GeneratedReleaseNotes.generate(&[]).unwrap(),
            "Explicit version release."
        );
        let original = commit(1, "feat: Add");
        let revert = commit(
            2,
            &format!("revert: Undo\n\nThis reverts commit {}.", original.sha),
        );
        assert_eq!(
            GeneratedReleaseNotes.generate(&[original, revert]).unwrap(),
            "Explicit version release."
        );
    }

    #[test]
    fn decisions_bind_inputs_copy_history_and_preserve_captured_notes() {
        let source = "b".repeat(40);
        let manifest = v("0.1.0");
        let explicit = v("2.0.0");
        let mut history = history(Some("1.0.0"), vec![commit(1, "feat: Add 🦀")]);
        let decision = SemanticRelease::default()
            .analyze(&source, &manifest, Some(&explicit), &history)
            .unwrap();
        history.commits[0].message = "fix: Mutated later".to_owned();
        assert_eq!(decision.version(), Some(&explicit));
        assert_eq!(decision.commits()[0].message, "feat: Add 🦀");
        decision
            .check(&source, &manifest, Some(&explicit), Some(&explicit))
            .unwrap();
        assert!(
            decision
                .check(&"c".repeat(40), &manifest, Some(&explicit), Some(&explicit))
                .is_err()
        );
        assert!(
            decision
                .check(&source, &v("0.2.0"), Some(&explicit), Some(&explicit))
                .is_err()
        );
        assert!(
            decision
                .check(&source, &manifest, Some(&v("2.0.1")), Some(&explicit))
                .is_err()
        );
        assert!(
            decision
                .check(&source, &manifest, Some(&explicit), None)
                .is_err()
        );
        let notes = decision.notes().unwrap();
        decision
            .validate_prepared(&source, &explicit, &notes.replace('\n', "\r\n"))
            .unwrap();
        assert!(
            decision
                .validate_prepared(&source, &explicit, "different")
                .is_err()
        );
        assert!(
            decision
                .validate_prepared(&source, &v("2.0.1"), notes)
                .is_err()
        );
        let encoded = serde_json::to_string(&decision).unwrap();
        let decoded: SemanticDecision = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, decision);
        let invalid = encoded.replace("\"version\":\"2.0.0\"", "\"version\":\"1.0.0\"");
        assert!(serde_json::from_str::<SemanticDecision>(&invalid).is_err());
    }

    #[test]
    fn no_eligible_history_is_not_a_releasable_decision_but_override_is() {
        let source = "b".repeat(40);
        let manifest = v("0.1.0");
        let history = history(None, vec![commit(1, "docs: Explain API")]);
        let service = SemanticRelease::default();
        let decision = service.analyze(&source, &manifest, None, &history).unwrap();
        assert!(!decision.releasable());
        assert!(decision.require_release().is_err());
        assert!(decision.notes().is_none());
        let explicit = v("1.2.3");
        let decision = service
            .analyze(&source, &manifest, Some(&explicit), &history)
            .unwrap();
        assert_eq!(decision.require_release().unwrap().0, &explicit);
        assert!(decision.notes().unwrap().contains("Explain API"));
        let empty = service
            .analyze(
                &source,
                &manifest,
                Some(&explicit),
                &SemanticHistoryData::default(),
            )
            .unwrap();
        assert_eq!(empty.notes(), Some("Explicit version release."));
    }

    #[test]
    fn invalid_history_and_tampered_decisions_are_rejected() {
        let source = "b".repeat(40);
        let manifest = v("0.1.0");
        let service = SemanticRelease::default();
        let good = history(Some("1.0.0"), vec![commit(1, "fix: Repair")]);
        assert!(service.analyze("bad", &manifest, None, &good).is_err());
        let mut invalid = good.clone();
        invalid.commits.push(invalid.commits[0].clone());
        assert!(service.analyze(&source, &manifest, None, &invalid).is_err());
        invalid = good.clone();
        invalid.last_release.as_mut().unwrap().tag = "v9.0.0".to_owned();
        assert!(service.analyze(&source, &manifest, None, &invalid).is_err());
        invalid = good.clone();
        invalid.commits[0].sha = "G".repeat(40);
        assert!(service.analyze(&source, &manifest, None, &invalid).is_err());
        let valid = service
            .analyze(&source, &manifest, None, &good)
            .unwrap()
            .into_data();
        let mut invalid = valid.clone();
        invalid.notes = Some("## Heading\n\n<!-- comment -->".to_owned());
        assert!(SemanticDecision::new(invalid).is_err());
        let mut invalid = valid.clone();
        invalid.version = Some(v("1.0.0"));
        assert!(SemanticDecision::new(invalid).is_err());
        let mut invalid = valid;
        invalid.version = None;
        assert!(SemanticDecision::new(invalid).is_err());
    }

    #[test]
    fn custom_services_are_composable_but_cannot_emit_unbound_or_empty_releases() {
        struct Analyzer;
        impl CommitAnalyzer for Analyzer {
            fn analyze(&self, _: &[SemanticCommit]) -> Result<Option<ReleaseType>> {
                Ok(Some(ReleaseType::Patch))
            }
        }
        struct Notes;
        impl SemanticNotes for Notes {
            fn generate(&self, _: &[SemanticCommit]) -> Result<String> {
                Ok("## Only a heading".to_owned())
            }
        }
        let service = SemanticRelease::new(
            Box::new(Analyzer),
            Box::new(SemanticVersionSelector::default()),
            Box::new(Notes),
        );
        assert!(
            service
                .analyze(
                    &"b".repeat(40),
                    &v("0.1.0"),
                    None,
                    &SemanticHistoryData::default()
                )
                .is_err()
        );
    }
}
