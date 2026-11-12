# Contributing

Read [Design](DESIGN.md) and [Agent Instructions](AGENTS.md) before making changes.

## Setup and Validation

Install [rustup](https://rustup.rs), Git, curl, and tar on your PATH. The toolchain file selects Rust 1.99.1 with rustfmt, Clippy, and LLVM tools; the crate requires Rust 1.99.1 or later. Install the pinned validation tools using the same Rust setup command as CI and release publishing:

```sh
cargo xtask setup
```

Setup keeps matching tool versions, installs missing or different versions of rumdl and typos-cli with Cargo, and downloads actionlint into Cargo's bin directory, which must be on PATH. Tool versions and installation commands live in `xtask/src/setup.rs`. For coverage, also install `cargo-llvm-cov` with `cargo install --locked cargo-llvm-cov --version 0.9.1`.

Run these commands from the repository root:

```sh
cargo test --locked --workspace
cargo xtask lint
cargo xtask package
cargo llvm-cov --locked --workspace --lcov --output-path target/lcov.info
```

Project code, fixtures, and maintenance commands are Rust. The standard GitHub upload action is a separate workflow step; Rust independently verifies its stored bytes through the public REST API before publication. Shared source/installed executable fixtures exercise preparation, rejected remote bytes, and successful verification before isolated publication. [GitHub Actions](docs/actions.md) documents permissions and live-service validation limits.

Tests and lint must pass for the exact revision before every push, including PR updates. Run package validation before committing changes to packaging or public entry points. Reuse final check results if the tested content/configuration has not changed; do not repeatedly run full suites during unrelated edits. Run focused tests while debugging, for example `cargo test --locked -p xtask docs::tests`.

The Package command uses `cargo package --locked --allow-dirty` so contributors can validate before committing; this does not authorize releasing a dirty tree. It inspects the archive allowlist, compiles its verified source, runs an isolated Rust consumer, installs the extracted archive's rcr executable, and runs the same real-process fixtures as source tests. Temporary consumers are deleted automatically; `.crate` files remain under ignored `target/package/`. No command above publishes or authenticates.

## Tests

Maintenance tests live alongside code in `xtask/src/`; they cover Markdown paragraph/title policies and YAML-aware workflow display names, including invalid cases, expressions, and unrelated machine fields. Library unit tests live beside their modules in `src/` and rustdoc examples exercise public composition. Do not add trivial API tests or fake release commands to inflate coverage.

Implementation tests belong in `tests/` and must exercise the real release engine against isolated Git remotes and a local simulated registry. Share source/installed executable fixtures. Cover lifecycle order, trusted edit boundaries, exact uploaded bytes, failed pushes, delayed visibility, and recovery after partial success. Optional artifact overrides must behave consistently when set, absent, or empty. No live publication, credential exchange, or registry mutation in tests.

Aim near 100% of project-owned runtime line/branch coverage where supported, including subprocess errors and meaningful edge cases; report gaps rather than hiding them with exclusions or trivial assertions. The Test workflow submits LCOV to Coveralls; runtime and maintenance code are measured together. Coveralls repository enablement, if required, is an external maintainer setting, not proof of local publication or authentication.

## Documentation and Tooling

Keep Markdown paragraphs on one source line. Human-readable Markdown headings and YAML workflow/job/step display names use title case; preserve proper names, acronyms, identifiers, and expressions. `cargo xtask lint` enforces these policies with a Markdown parser and a YAML-aware checker, alongside Markdown structure, spelling, Rust, and Actions checks. Review narrow vocabulary exceptions in `.typos.toml`; do not suppress entire documentation trees.

Use separate Test, Lint, and Package workflows and retain Coveralls coverage. Matrices use explicit compact `matrix.entry` objects, never `include`/`exclude` or Cartesian axes. Verify current stable versions when selecting/updating dependencies or tools while preserving the supported Rust floor. Keep [Design](DESIGN.md) current rather than retaining obsolete decisions.

## Pull Requests

For artwork changes, preserve the editable SVG and PNG, licensing, README-first placement, and packaged assets. Follow [Artwork](DESIGN.md#artwork) and inspect full-size and README-scale renders.

Inspect status, branches, and worktrees first. Work on `<username>/<topic>` branches from `master`, for example `dblock/implementation`; use a separate worktree only for parallel tasks, never to avoid preserving unrelated changes. Prefer one coherent commit per PR; squash task-owned iterations before pushing. Allow squash or rebase merges in repository settings, but use squash when merging for @dblock; merge commits are disabled. No direct development commits/pushes or force-pushes to `master`.

Task-owned topic commits may be amended without asking again. Before rewriting, verify ownership, record the remote tip, reconcile remote changes, and run final local tests/lint. Push only with `--force-with-lease=<branch>:<recorded-sha>`; never rewrite someone else's work or force-push `master`. Include `Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>` in AI-assisted commits, and disclose AI authorship on behalf of @dblock in AI-authored PRs/comments.

Update affected documentation and add changelog entries for features, improvements, and fixes, not minor typos or the initial scaffold. Append entries above the final contribution placeholder, oldest first within the newest-first release sections. Use `- [#123](https://github.com/dblock/rust-crate-release/pull/123): Description - [@author](https://github.com/author).` Predict the next issue/PR number before opening, then verify/correct it. Use closing issue references in PRs rather than manually closing issues.

Before bumping versions, check whether the current version is actually published; ask if status is unknown. Unreleased `0.1.0` stays `0.1.0`. See the exact version policy in [Agent Instructions](AGENTS.md). Breaking changes to released behavior, including `0.x`, need migration examples in `UPGRADING.md`, linked from README, identifying source/target versions and the introducing PR.

After a PR merges, fetch with pruning and verify GitHub's merged state, clean checkouts, and any dependent stacked PRs. Switch to `master`, fast-forward, remove only clean task-owned parallel worktrees, then delete merged local topic branches. Use `git branch -D` only after verifying squash/rebase merge and that no unmerged/dependent work will be lost; never remove dirty worktrees, active PR branches, or `master`. Report blocked cleanup rather than discarding work.

Do not merge, publish, create tags/releases, set up credentials/trusted publishing, or change release protections without explicit authorization. There is no direct-push release exception configured. See [Releasing](RELEASING.md); written instructions do not grant authorization.
