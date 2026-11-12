# Working on rust-crate-release

Read [CONTRIBUTING.md](CONTRIBUTING.md) and [DESIGN.md](DESIGN.md) first.

## Scope and Invariants

Port the Ruby/npm release tools with full first-implementation parity, using Rust-native typed workflows, provider boundaries, and exact Cargo artifacts. The numeric/semantic release engine, hooks, snapshots, Markdown/changelogs, providers, diagnostics, and isolated testing helpers are available; CLI and Actions wrappers remain pending. The library lives in `src/`, real engine tests in `src/engine/tests.rs`, maintenance tooling/tests in `xtask/src/`, and future shared CLI integrations in `tests/`. One distributable crate and a private maintenance workspace member are intentional; multi-crate orchestration is outside the peers' existing scope.

Rust 1.99.1 or later is supported; development is pinned in `rust-toolchain.toml`. Prefer current stable public APIs when adding/updating dependencies, preserve floor compatibility, and commit `Cargo.lock`. Use pulldown-cmark byte offsets rather than re-rendering Markdown; use public SemVer/serde/thiserror, Cargo metadata, TOML-editing, SHA-256, HTTP, and glob APIs. Never replace the retained-archive upload protocol with `cargo publish`. Validate the actual `.crate` and prove uploaded bytes at a loopback registry boundary without real credentials.

Keep release identity separate from execution metadata and credentials. Trusted before/after preparation and next hooks finish before literal staging; later validation must not silently stage arbitrary files. Validate archive VCS identity against the prepared commit. GitHub publication requires a real durable uploader before registry mutation. Recovery reconciles real outputs, not completion flags; never overwrite published versions/tags, automatically retry ambiguous uploads, rewind history, or discard unrelated changes. Verify against the captured released version/archive after next-development edits.

## Validation and Documentation

Run `cargo test --locked --workspace` and `cargo xtask lint` for the exact revision before every push. Run `cargo xtask package` before committing packaging or public-entry changes; this builds and inspects a real archive and runs an extracted-artifact consumer. Use focused tests such as `cargo test --locked -p xtask docs::tests` during debugging; avoid repeated intermediate full suites. Final check results may be reused until tested content/configuration changes.

Use `cargo llvm-cov --locked --workspace --lcov --output-path target/lcov.info` for coverage. Aim near 100% of project-owned runtime code during implementation; cover meaningful failures and installed subprocesses, not trivial constants or excluded logic. Runtime tests need real isolated Git remotes, simulated publication, hook order/failures, concurrency and failed pushes, bounded visibility, and explicit-version verification. Report unexercised production credentials and unavailable branch instrumentation honestly.

Keep separate Test, Lint, and Package workflows and applicable Coveralls submission. Use explicit compact `matrix.entry` lists without `include`/`exclude` or generated Cartesian combinations. `cargo xtask lint` checks Rust formatting, Clippy, rustdoc, Markdown structure/spelling, Actions, and parsed documentation/YAML title policies. Keep human-readable headings and workflow/job/step display names in title case while preserving proper names, acronyms, expressions, and identifiers. Keep Markdown paragraphs on one source line.

Update documentation and linked changelog entries for meaningful features/improvements/fixes, with actual PR numbers and contributor attribution; omit minor typos and the initial scaffold. Predict the number before opening a PR and verify afterward. Follow [CONTRIBUTING.md](CONTRIBUTING.md) for entry ordering. Keep DESIGN focused on current architecture; remove obsolete proposals. Breaking changes to released behavior need `UPGRADING.md` with source/target versions, short before/after examples, required steps, and the introducing PR, linked from README.

## Versioning

Before any bump, verify whether the current version is published in crates.io; ask when status cannot be established. Keep an unreleased version unchanged, including the initial `0.1.0`. If already released, bump in the same PR as the change and synchronize Cargo metadata, lockfile, and Next changelog heading. Features in `0.x` increment minor/reset patch; features in `1.x` and later increment minor/preserve patch. Breaking changes in `0.x` increment minor/reset patch without asking for a major; in `1.x` and later ask before choosing a major bump. Fixes use a patch bump. Document migration even for breaking `0.x` changes.

## Git and Release Safety

Inspect status, branches, and worktrees; preserve unrelated changes. Use `<username>/<topic>` branches from `master`, and worktrees only for parallel tasks. Keep one coherent commit per PR, allowing task-owned amendments without renewed permission. Before rewriting, verify ownership, record/reconcile the remote tip, validate, and use an explicit `--force-with-lease=<branch>:<recorded-sha>`. Never rewrite another task's work, directly develop/push on `master`, or force-push `master`.

Repository settings allow squash/rebase but not merge commits; use squash merges for @dblock. No merge, publication, tags/releases, credential setup, trusted publishing, or release-protection changes without explicit authorization. No release-automation direct-push exception is configured. [RELEASING.md](RELEASING.md) is provisional and does not authorize release actions.

Include the Copilot co-author trailer in AI-assisted commits and disclose AI authorship on behalf of @dblock in PRs/comments. Use PR closing references rather than manually closing issues. After merge, fetch/prune, verify GitHub's merged state, inspect clean checkouts/dependents, switch to updated `master`, and remove clean task-owned worktrees/merged branches. Force-delete a local branch only after verifying squash/rebase merge and no dependent/unmerged work loss; never remove dirty worktrees, active PRs, or `master`.
