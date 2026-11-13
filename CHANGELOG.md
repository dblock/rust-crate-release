# Changelog

This changelog follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Changed

- [#36](https://github.com/dblock/rust-crate-release/pull/36): Centralize simulator endpoint validation, reuse single-read archive verification across hosting and local publication adapters, and add repeatable test-suite timings and function benchmarks without changing public APIs - [@dblock](https://github.com/dblock).
- [#38](https://github.com/dblock/rust-crate-release/pull/38): Run isolated CLI fixture scenarios with four workers, retain every validation and failure assertion, reuse fixture Git handles, and report package phase and scenario timings - [@dblock](https://github.com/dblock).

### Fixed

- [#37](https://github.com/dblock/rust-crate-release/pull/37): Make Criterion an opt-in benchmark dependency, check benchmark features during lint validation, exercise package-metadata fixture dependencies, and fail contributor builds on compiler or Cargo lint warnings - [@dblock](https://github.com/dblock).

## [0.1.2] - 2026-11-12

### Added

- [#35](https://github.com/dblock/rust-crate-release/pull/35): Expose workspace test coverage through a README badge and document coverage artifacts and Coveralls repository claiming - [@dblock](https://github.com/dblock).

### Changed

- [#29](https://github.com/dblock/rust-crate-release/pull/29): Reduce release compilation overhead with verified prebuilt validation tools, Cargo dependency caching, and shared extracted-archive build dependencies while retaining every validation phase - [@dblock](https://github.com/dblock).
- [#30](https://github.com/dblock/rust-crate-release/pull/30): Cancel superseded validation runs, apply matrix toolchains throughout test jobs with explicit fixture validation components, and simplify setup and contributor guidance - [@dblock](https://github.com/dblock).
- [#31](https://github.com/dblock/rust-crate-release/pull/31): Use Keep a Changelog for this repository's release notes and maintenance release configuration - [@dblock](https://github.com/dblock).

### Fixed

- [#32](https://github.com/dblock/rust-crate-release/pull/32): Permit the retained Unreleased comparison during release lint and avoid executing a recently written version-probe test fixture on Linux - [@dblock](https://github.com/dblock).

## [0.1.1] - 2026-11-12

### Fixed

- [#27](https://github.com/dblock/rust-crate-release/pull/27): Install rustfmt and Clippy for release validation and retain source inputs in recovery artifacts instead of executing them after notification - [@dblock](https://github.com/dblock).
- [#28](https://github.com/dblock/rust-crate-release/pull/28): Install self-release validation tools through an optional Actions setup command and report failed internal steps to failure callbacks - [@dblock](https://github.com/dblock).

## [0.1.0] - 2026-11-12

### Added

- [#2](https://github.com/dblock/rust-crate-release/pull/2): Add typed workflows, lifecycle hooks, execution providers, source-preserving changelogs, and semantic release analysis - [@dblock](https://github.com/dblock).
- [#4](https://github.com/dblock/rust-crate-release/pull/4): Add cross-platform exact-artifact Cargo/crates.io providers, guarded Git/hosting operations, durable diagnostics, semantic history discovery, and isolated consumer testing helpers - [@dblock](https://github.com/dblock).
- [#5](https://github.com/dblock/rust-crate-release/pull/5): Compose typed numeric/semantic release workflows with durable pre-publication artifacts, independent reconciliation, real validation, and explicit-version verification - [@dblock](https://github.com/dblock).
- [#6](https://github.com/dblock/rust-crate-release/pull/6): Add the rcr CLI, trusted configuration, explicit source-input transport, read-only verification, and shared source/installed executable release fixtures - [@dblock](https://github.com/dblock).
- [#7](https://github.com/dblock/rust-crate-release/pull/7): Add reusable/composite GitHub Actions, awaited durable archive preservation before publication, hidden diagnostics, and machine-output-safe credential masking - [@dblock](https://github.com/dblock).
- [#8](https://github.com/dblock/rust-crate-release/pull/8): Add an original open-crate release logo, editable and raster assets, and rendering/packaging guidance - [@dblock](https://github.com/dblock).
- [#9](https://github.com/dblock/rust-crate-release/pull/9): Configure repository release preparation for its Intridea changelog and full workspace, distribution, and artifact-support validation - [@dblock](https://github.com/dblock).
- [#10](https://github.com/dblock/rust-crate-release/pull/10): Add Rust Actions commands and independent remote recovery-artifact verification before publication - [@dblock](https://github.com/dblock).
- [#11](https://github.com/dblock/rust-crate-release/pull/11): Document 0.1.0 installation, project setup, trusted publishing, file-editing lifecycle hooks, release operation, and recovery - [@dblock](https://github.com/dblock).
- [#13](https://github.com/dblock/rust-crate-release/pull/13): Add scoped global literal and regex Markdown section substitutions with capture references - [@dblock](https://github.com/dblock).
- [#14](https://github.com/dblock/rust-crate-release/pull/14): Support character classes, ranges, negation, and escaping in semantic release-rule selectors - [@dblock](https://github.com/dblock).
- [#15](https://github.com/dblock/rust-crate-release/pull/15): Add a changelog-disabled mode with retained numeric or semantic release notes and no changelog file access - [@dblock](https://github.com/dblock).
- [#16](https://github.com/dblock/rust-crate-release/pull/16): Add typed hooks around authentication, local tagging, and separate development commit/push boundaries - [@dblock](https://github.com/dblock).
- [#17](https://github.com/dblock/rust-crate-release/pull/17): Load trusted project Rust runners through CLI configuration and stock Actions while preserving typed hooks and artifact verification - [@dblock](https://github.com/dblock).
- [#18](https://github.com/dblock/rust-crate-release/pull/18): Add terminal success/failure callbacks and credential-free actual Actions step/run context, including external setup failures - [@dblock](https://github.com/dblock).
- [#19](https://github.com/dblock/rust-crate-release/pull/19): Configure Actions toolchains, Git identity, and trusted phase commands with exact-byte publication verification - [@dblock](https://github.com/dblock).
- [#20](https://github.com/dblock/rust-crate-release/pull/20): Add a configured consumer release-testing facade with isolated registry/hosting, project runners, and exact remote-file assertions - [@dblock](https://github.com/dblock).
- [#21](https://github.com/dblock/rust-crate-release/pull/21): Add a manual SDK self-release caller and source-owned runner maintaining released documentation pins in the next-development commit - [@dblock](https://github.com/dblock).
- [#23](https://github.com/dblock/rust-crate-release/pull/23): Make standard releases configuration-free with inferred default branches and GitHub repositories, optional overrides, and an explicit hosted-release opt-out - [@dblock](https://github.com/dblock).
- [#22](https://github.com/dblock/rust-crate-release/pull/22): Simplify release adoption with step-by-step README setup, Cargo first publication, trusted publishing, version examples, and practical configuration, changelog, and recovery guides, with advanced hooks documented separately - [@dblock](https://github.com/dblock).
- [#25](https://github.com/dblock/rust-crate-release/pull/25): Remove the logo background and shadow and align its visible height with sibling release-tool logos - [@dblock](https://github.com/dblock).
- [#24](https://github.com/dblock/rust-crate-release/pull/24): Automatically detect supported handwritten changelogs or skip missing files, reject unknown or ambiguous formats before source edits, and preserve explicit overrides and semantic generated notes - [@dblock](https://github.com/dblock).

[Unreleased]: https://github.com/dblock/rust-crate-release/compare/v0.1.2...master
[0.1.2]: https://github.com/dblock/rust-crate-release/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/dblock/rust-crate-release/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/dblock/rust-crate-release/releases/tag/v0.1.0
