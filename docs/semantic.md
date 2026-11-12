# Semantic Release Analysis

`SemanticRelease` composes separate `CommitAnalyzer`, `SemanticVersionPolicy`, and `SemanticNotes` services. Its `analyze(source_sha, manifest_version, input_override, history)` method consumes immutable supplied history and returns an owned `SemanticDecision`; it neither discovers Git history nor edits files. Git adapters implement `SemanticHistory`, rejecting shallow history and selecting the highest numeric reachable stable `vX.Y.Z` tag, not the lexicographically greatest or nearest tag.

## Conventional Commits

`ConventionalCommits` recognizes `type(scope): subject` with optional scopes and `!`, full multiline bodies, and `BREAKING CHANGE:` or `BREAKING-CHANGE:` trailers. Features select minor; fixes, performance changes, and uncancelled reverts select patch; breaking changes select major even from `0.x`. Other commits alone do not release. Original/revert pairs cancel, reverts of reverts restore the original effect, and ambiguous abbreviated reverted SHAs fail.

`ConventionalCommits::new(rules)` replaces defaults for commits matching any rule. Optional type, scope, subject, breaking, and revert selectors are combined; string selectors support literal text plus `*` and `?`. Matching rules select their highest release type, while `release: None` suppresses a change. Cancelled commits cannot be restored by a rule.

## Version Selection

`SemanticVersionSelector` defaults to a first eligible release of `1.0.0`, independent of the manifest version. Its constructor accepts another stable first version. Subsequent versions increment the selected component and reset lower components. An explicit stable override may replace the classification but must exceed the prior release. Prereleases, build metadata, and numeric overflow fail.

Without eligible commits or an override, analysis returns a non-releasable decision; workflow callers must call `require_release()` and fail before edits rather than report a successful skip. Next-development policy belongs to the workflow and is off by default for semantic releases.

## Captured Decisions and Notes

`SemanticDecision` binds the source SHA, original manifest version and override, previous tag/version, full commit messages, classification, selected release version, and captured generated notes. Accessors are immutable; serialization and validated deserialization support invocation-local transport. `check` verifies original inputs, and `validate_prepared` verifies prepared identity and notes without reanalysis against a new release tag or advanced manifest. This is not a cache or authority to recover another invocation from downloaded artifacts.

`GeneratedReleaseNotes` produces deterministic Markdown groups for breaking changes, features, bug fixes, performance improvements, and other changes. It escapes Markdown text, includes short SHAs, preserves history order within groups, removes duplicate subjects per scope, and omits cancelled pairs. An explicit release with no remaining changes receives a short fallback note. Custom analyzers and notes generators are independent; all selected decisions still require meaningful notes. Pass captured notes to `GeneratedChangelog` during preparation and retain those same notes in release artifacts.
