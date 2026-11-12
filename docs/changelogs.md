# Changelog Editing

The `Changelog` trait separates pure source transformations from explicit `Path`-based file edits. `release` returns `ChangelogEdit { source, notes }`; `next` prepares development notes. Both validate before writing through `release_file` or `next_file`. Dates are real Gregorian dates supplied by the caller as `YYYY/MM/DD`; prereleases and build metadata are unsupported.

## Handwritten Changelogs

`KeepAChangelog::default()` releases a unique `[Unreleased]` heading as `[X.Y.Z] - YYYY-MM-DD`, preserving categories and historical source. Its default next section is empty. `KeepAChangelog::new(ChangelogTemplate::new(placeholder, sections)?)` optionally removes exact contribution prompts and empty categories during release and restores the configured categories and prompts during next-development preparation.

`IntrideaChangelog::default()` releases a unique `X.Y.Z (Next)` heading as `X.Y.Z (YYYY/MM/DD)`. At least one real `- Your contribution here.` prompt is required; all matching prompts outside code, quotes, and HTML examples are removed. `IntrideaChangelog::new(prompt)?` selects another single-line prompt. Next-development preparation carries direct category headings and adds a prompt below each, without copying historical notes or deeper subsections. Heading levels are retained.

Matching existing release sections are reused without changing their dates; supplied captured notes must match. Duplicate or malformed release headings, ambiguous pending sections, empty notes, conflicting next sections, and non-increasing next versions fail. Keep a Changelog updates an optional HTTP comparison definition from `vOLD...BRANCH` to `vNEW...BRANCH` and adds the released comparison. Unprefixed numeric tags and other tag prefixes are also supported; next-development preparation verifies the comparison baseline. Duplicate, multiline, and unsupported link definitions fail rather than being guessed.

## Generated Changelogs

`GeneratedChangelog` requires meaningful captured notes through `release(..., Some(notes))`. Empty source or a missing file starts with `# Changelog`; a dated level-two section is inserted before existing level-two history. Matching notes reuse the existing section without date changes. Conflicting notes and duplicate release headings fail before writing. `next` leaves the document unchanged and never creates a fake contribution placeholder.

## Markdown Boundaries

`MarkdownDocument` owns UTF-8 source and byte offsets obtained from the Markdown parser. Only actual top-level ATX headings define editable sections; fenced code, lists, quotes, HTML examples, and setext headings cannot impersonate release headings. Unrelated Markdown and LF or CRLF line endings remain intact; mixed or bare-carriage-return line endings fail.

`MarkdownSection::new("## Exact Heading")?` selects one complete exact heading line. `replace(source, body)` replaces its body; `replace_text(source, search, replacement)` replaces the first literal match only within that body. Corresponding file helpers accept `&Path`. Markdown structure alone is not meaningful release content: headings, HTML, comments, link definitions, thematic breaks, and contribution prompts do not count, while code and images with descriptions do.

Use `replace_all_text` for every literal match, or `replace_regex` for global Rust regular-expression matching with capture references such as `${version}` or `$1`. Their `_file` helpers validate the full edit before writing. Matches are limited to the selected body, including nested subsections/code examples, never its heading or surrounding sections. File line endings and unchanged files are preserved; invalid regexes or unsupported replacement line endings fail without writing. Rust regex syntax intentionally excludes backreferences and look-around.
