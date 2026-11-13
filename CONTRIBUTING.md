# Contributing

Read [Design](DESIGN.md) and [Agent Instructions](AGENTS.md) before making changes.

## Setup and Validation

Install [rustup](https://rustup.rs), Git, curl, and tar on your PATH. The toolchain file selects Rust 1.99.1 with rustfmt, Clippy, and LLVM tools; the crate requires Rust 1.99.1 or later. Install the pinned validation tools using the same Rust setup command as CI and release publishing:

```sh
cargo xtask setup
```

Setup keeps matching tool versions and downloads actionlint into Cargo's bin directory, which must be on PATH. On Linux x86_64 (including the release runner), rumdl and typos use pinned upstream binaries with SHA-256 verification instead of compiling their dependencies; other platforms install them with Cargo. Tool versions, checksums, and installation commands live in `xtask/src/setup.rs`. For coverage, also install `cargo-llvm-cov` with `cargo install --locked cargo-llvm-cov --version 0.9.1`.

Before every push, run tests and lint from the repository root. Also run package validation for packaging or public-entry changes:

```sh
cargo test --locked --workspace
cargo xtask lint
cargo xtask package
```

Checks must pass for the exact revision, including PR updates; run package validation before committing affected changes. Reuse results until tested content/configuration changes. Use focused tests while debugging, for example `cargo test --locked -p xtask docs::tests`.

The repository's Cargo configuration sets `build.warnings = "deny"`, so compiler and Cargo lint warnings fail local and CI commands instead of passing silently. This is a contributor-build policy, not a lint policy imposed on consumers of the published crate. Lint validation enables all workspace features and targets, including the opt-in benchmark target.

The Package command uses `cargo package --locked --allow-dirty` so contributors can validate before committing; this does not authorize releasing a dirty tree. It inspects the archive allowlist, compiles its verified source, runs an isolated Rust consumer, installs the extracted archive's rcr executable, and runs the same real-process fixtures as source tests. Package phases and individual CLI scenarios report elapsed times, with at most four scenarios running concurrently per fixture-suite invocation. Each scenario retains its own temporary projects, remotes, and publication destinations; errors and assertion panics fail the suite after all scenarios finish. Extracted consumers and installed binaries remain temporary; their Cargo builds share `target/archive-consumer/` to reuse compiled dependencies across release/development validation without skipping source compilation or fixture checks. Temporary consumers are deleted automatically; `.crate` files remain under ignored `target/package/`. No command above publishes or authenticates.

## Tests

Maintenance tests live alongside code in `xtask/src/`; they cover Markdown paragraph/title policies and YAML-aware workflow display names, including invalid cases, expressions, and unrelated machine fields. Library unit tests live beside their modules in `src/` and rustdoc examples exercise public composition. Do not add trivial API tests or fake release commands to inflate coverage.

Implementation tests belong in `tests/` and must exercise the real release engine against isolated Git remotes and a local simulated registry. Share source/installed executable fixtures. Cover lifecycle order, trusted edit boundaries, exact uploaded bytes, failed pushes, delayed visibility, and recovery after partial success. Optional artifact overrides must behave consistently when set, absent, or empty. No live publication, credential exchange, or registry mutation in tests.

For local coverage, run `cargo llvm-cov --locked --workspace --lcov --output-path target/lcov.info`. Aim near 100% of project-owned runtime line/branch coverage where supported, including subprocess errors and meaningful edge cases; report gaps rather than hiding them with exclusions or trivial assertions. The Test workflow measures runtime and maintenance code together and submits LCOV to Coveralls. Tests use isolated adapters, not live publication or credential exchange; see [GitHub Actions](docs/actions.md#validation-limits) for service-validation limits.

The README coverage badge links to the [master coverage report](https://coveralls.io/github/dblock/rust-crate-release?branch=master); the Coverage job also retains `lcov.info` in its `coverage` artifact. If Coveralls reports that the repository is unclaimed, a repository administrator must sign in with GitHub and select **Claim** in the repository's Coveralls settings to enable commit statuses. Report uploads and the badge work before claiming.

## Benchmarks

Run `cargo xtask bench-tests` for separate library, CLI, release-runner, maintenance, and doctest wall times. It precompiles tests, then runs each suite once in sequence with its normal internal parallelism, saving seconds to `target/benchmarks/test-suites.json`. Timings include Cargo startup and, for doctests, rustdoc compilation; they are observations rather than statistical performance assertions. Run on an otherwise idle machine and repeat to assess variance.

Run `cargo bench --locked -p xtask --features benchmarks --bin operations` for optimized Criterion benchmarks of Markdown parsing, changelog release/next transformations, default and glob-rule semantic analysis, archive opening/verification, and local publication reconciliation. The `benchmarks` feature enables Criterion and its binary benchmark harness; ordinary maintenance commands and workspace tests do not compile them. Documents and commit histories use 10, 100, and 1,000 entries. The archive fixture has a deterministic 256 KiB payload; fixture generation, Cargo packaging, and initial publication are outside the measured loops. Archive and reconciliation operations include filesystem I/O, temporary extraction, and offline `cargo metadata` subprocesses, not just hashing; archive throughput counts compressed bytes. No benchmark contacts a production registry or host.

Criterion uses 20 samples, a one-second warmup, and a two-second target measurement window per case, extending that window when needed for slow operations. Save a baseline with `cargo bench --locked -p xtask --features benchmarks --bin operations -- --save-baseline before`, then compare a later revision with `-- --baseline before` on the same machine and toolchain. Results live under Cargo's target directory in `criterion/`. Benchmarks belong to the private maintenance package, do not ship in the crate, and are not release-validation gates; compare them separately from debug-profile test timings.

## Documentation and Tooling

Keep Markdown paragraphs on one source line. Human-readable Markdown headings and YAML workflow/job/step display names use title case; preserve proper names, acronyms, identifiers, and expressions. `cargo xtask lint` enforces these policies with a Markdown parser and a YAML-aware checker, alongside Markdown structure, spelling, Rust, and Actions checks. Review narrow vocabulary exceptions in `.typos.toml`; do not suppress entire documentation trees.

Use separate Test, Lint, and Package workflows and retain Coveralls coverage. Matrices use explicit compact `matrix.entry` objects, never `include`/`exclude` or Cartesian axes. Verify current stable versions when selecting/updating dependencies or tools while preserving the supported Rust floor. Keep [Design](DESIGN.md) current rather than retaining obsolete decisions.

Validation workflows cancel superseded runs for the same workflow/ref; release workflows must remain non-canceling. Matrix toolchain selection applies to the whole test job, including dependency-cache metadata and child Cargo commands, not only its final test step. Install rustfmt and Clippy explicitly: test, coverage, and package fixtures invoke real release validation, and an explicit toolchain override does not install components from `rust-toolchain.toml`.

## Pull Requests

For artwork changes, preserve the editable SVG and PNG, licensing, README-first placement, and packaged assets. Follow [Artwork](DESIGN.md#artwork) and inspect full-size and README-scale renders.

Inspect status, branches, and worktrees first. Work on `<username>/<topic>` branches from `master`, for example `dblock/implementation`; use a separate worktree only for parallel tasks, never to avoid preserving unrelated changes. Prefer one coherent commit per PR; squash task-owned iterations before pushing. Allow squash or rebase merges in repository settings, but use squash when merging for @dblock; merge commits are disabled. No direct development commits/pushes or force-pushes to `master`.

Task-owned topic commits may be amended without asking again. Before rewriting, verify ownership, record the remote tip, reconcile remote changes, and run final local tests/lint. Push only with `--force-with-lease=<branch>:<recorded-sha>`; never rewrite someone else's work or force-push `master`. Include `Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>` in AI-assisted commits, and disclose AI authorship on behalf of @dblock in AI-authored PRs/comments.

Update affected documentation and add changelog entries for features, improvements, and fixes, not minor typos or the initial scaffold. This repository uses Keep a Changelog: append entries under `## [Unreleased]` in the appropriate `### Added`, `### Changed`, or `### Fixed` category, oldest first within each category. Release sections remain newest first; no contribution placeholder is required. Use `- [#123](https://github.com/dblock/rust-crate-release/pull/123): Description - [@author](https://github.com/author).` Predict the next issue/PR number before opening, then verify/correct it. Use closing issue references in PRs rather than manually closing issues.

Before bumping versions, check whether the current version is actually published; ask if status is unknown. Unreleased `0.1.0` stays `0.1.0`. See the exact version policy in [Agent Instructions](AGENTS.md). Breaking changes to released behavior, including `0.x`, need migration examples in `UPGRADING.md`, linked from README, identifying source/target versions and the introducing PR.

After a PR merges, fetch with pruning and verify GitHub's merged state, clean checkouts, and any dependent stacked PRs. Switch to `master`, fast-forward, remove only clean task-owned parallel worktrees, then delete merged local topic branches. Use `git branch -D` only after verifying squash/rebase merge and that no unmerged/dependent work will be lost; never remove dirty worktrees, active PR branches, or `master`. Report blocked cleanup rather than discarding work.

Do not merge, publish, create tags/releases, set up credentials/trusted publishing, or change release protections without explicit authorization. Check current repository protections before a separately authorized release; [Releasing](RELEASING.md) explains the required ordinary pushes but does not grant authorization.
