# CLI and Configuration

## Commands

Install `cargo install --locked rust-crate-release --version 0.1.0` with Rust 1.99.1 or later. To build a reviewed checkout instead, use `cargo install --locked --path .`. `rcr --version` prints the tool version; `--release-version` selects your crate's release version, not the tool version. Every successful operation prints exactly one JSON object to stdout; progress, validators, summaries, and errors use stderr. Exit codes are 0 for success or bounded visibility timeout, 1 for operational failure, and 2 for invalid command syntax.

```sh
rcr prepare --save-inputs .git/rcr/source-inputs.json
rcr release --inputs .git/rcr/source-inputs.json
rcr verify --name example-crate --release-version 0.1.0 --archive .git/rcr/example-crate-0.1.0.crate
```

These are real release operations, not dry runs. Preparation edits and validates source, commits, packages, creates a local tag, and retains a bundle; it does not publish, push, create a hosted release, or advance development. Release repeats preparation against actual outputs, preserves the artifact, publishes exact bytes, atomically pushes, optionally hosts and advances development, then verifies. For token-free automated releases, follow [GitHub Actions and Trusted Publishing](actions.md). See [Release Workflows](releases.md) for typed composition.

For example, standalone verification returns:

```json
{"name":"example-crate","operation":"verify","verification":{"attempts":6,"status":"not_visible"},"version":"0.1.0"}
```

Object key order is not a public contract. Workflow phases and configured validation argv execute in their documented order, without sorting or deduplication. Repeated validation commands are intentional and run repeatedly. Paths are filesystem paths relative to `--root`, which defaults to the current directory; absolute paths remain absolute. No source-coordinate or location-unit output exists. `--config` selects a file relative to that root; otherwise an absent `rcr.toml` selects defaults, while a missing explicitly selected file is an error.

`prepare` and `release` accept `--release-version`, optional `--archive`, `--inputs`, and `--save-inputs`. An explicit nonempty archive wins over nonempty `CRATE_ARCHIVE`; empty optional values behave as unset. A supplied invalid archive fails instead of silently rebuilding. Without an override, a matching retained archive is reused before building. Next-development validation always removes inherited `CRATE_ARCHIVE`.

`verify` requires `--name`, `--release-version`, and `--archive`. It reads configuration and the retained archive, queries/downloads that explicit published version, and compares identity, checksum, and bytes. It does not construct a Git repository, discover the current manifest version, write receipts, authenticate for publication, or mutate release outputs. Release-only flags are rejected by the parser.

## Local Publication

Run from the root of a clean Git checkout on the configured default branch, with full history and HEAD matching its remote. Commit a reviewed `Cargo.lock`, meaningful changelog notes, and the intended stable manifest version first. Configure your Git author and push authentication. `rcr.toml` defaults to branch `master` and remote `origin`; set `default-branch` for projects using another branch.

Prepare and inspect the captured release before supplying credentials:

```sh
rcr prepare --save-inputs .git/rcr/source-inputs.json
```

Default validation runs formatting, locked tests, and Clippy; packaging also compiles actual-archive consumers. Dependencies must be cached before offline packaging. Inspect the retained `.crate`, Git bundle, changelog, local release commit/tag, and `.git/rcr/release.json`. For linked worktrees, use `git rev-parse --git-path rcr` to locate the metadata directory instead of assuming `.git` is a directory.

Create a narrowly scoped crates.io API token and supply it privately as `CARGO_REGISTRY_TOKEN` in your local shell. rcr does not read tokens saved by `cargo login`; do not put a literal token in commands, shell history, documentation, or Git. If `github-repository` is set, also provide GitHub authentication through `gh auth login` or private `GH_TOKEN`/`GITHUB_TOKEN`. Registry authentication and Git/GitHub authentication are separate.

Once ready to publish, run:

```sh
rcr release --inputs .git/rcr/source-inputs.json
unset CARGO_REGISTRY_TOKEN
```

This publishes the exact retained archive, pushes its release commit/tag atomically, hosts the release if configured, validates/pushes the next development version, and verifies the released version. Branch and tag protections must permit those ordinary pushes. Remove the token from your environment even if the command fails; inspect [partial-success recovery](recovery.md) before retrying.

For a new crate this local publication establishes the version required by crates.io before configuring trusted publishing. For an existing crate, use the [Actions setup](actions.md) to avoid local registry tokens on subsequent releases. Local artifacts alone are not sufficient for GitHub execution: the composite action requires independently verified durable preservation before publication.

## Captured Source Inputs

`--save-inputs` atomically writes a private JSON file immediately after source analysis, before preparation edits. It contains immutable source identity/date, the explicit override, and any source-bound semantic decision, never workflow phases or credentials. Existing identical data is reusable; different contents and symbolic-link destinations are rejected. Keep this file under Git metadata or outside the checkout so it does not make the source dirty.

Separate preparation/publication commands must explicitly pass `--inputs`; ordinary commands never discover or restore diagnostic receipts. An omitted release override is taken from those explicit inputs; a conflicting override fails engine validation. The engine rechecks ancestry, timestamp, identity, tags, and actual outputs. Semantic analysis is not repeated against the newly created tag. Failed validation preserves edits; inspect and explicitly repair/validate/commit the checkout before retrying with reviewed original inputs.

Inside GitHub Actions, source-input transport is bound to the repository, workflow ref, run ID, and attempt. Another run/attempt is rejected; downloaded state is not an automatic recovery mechanism. Fresh dispatches capture their own inputs. [GitHub Actions](actions.md) supplies durable pre-publication uploading; the bare CLI intentionally refuses GitHub publication without independently verified preservation. The hidden `--github-artifact-id` option selects an immutable upload whose real metadata and ZIP contents must match the current run/attempt and retained recovery files; an ID alone never authorizes publication.

The composite action calls the Rust `rcr actions guard`, `rcr actions prepare`, and `rcr actions release` commands. They enforce manual/default-branch hosted OIDC execution, use Git's rcr directory regardless of configured artifact storage, and write single-line outputs to `GITHUB_OUTPUT`. Preparation consumes optional `RCR_CONFIG`/`RCR_VERSION`; release consumes the same explicit saved inputs and `RCR_ARTIFACT_ID` from the intervening standard upload action. Release also honors `RCR_CONFIG`. These commands retain the ordinary one-JSON-object stdout contract.

## Configuration

`rcr.toml` is trusted data, not a dynamically loaded Rust module or shell script. Unknown keys, invalid enums/timezones/retry bounds, empty validation argv, and incompatible workflow/changelog selections fail. Validators execute argv directly, without a shell. Use a project-owned Rust runner for typed hooks or custom providers.

```toml
workflow = "numeric"
manifest = "Cargo.toml"
default-branch = "master"
remote = "origin"
timezone = "UTC"
changelog = "CHANGELOG.md"
changelog-format = "keep-a-changelog"
next-version = "patch"
validation = [
  ["cargo", "fmt", "--all", "--", "--check"],
  ["cargo", "test", "--locked"],
  ["cargo", "clippy", "--locked", "--all-targets", "--", "-D", "warnings"],
]
verification-attempts = 6
verification-delay-seconds = 5
```

These are numeric defaults. `manifest` explicitly selects one package, including a workspace member. Changelog paths must stay inside the Git root without symbolic-link traversal. `artifact-directory` optionally selects storage; the global `--artifact-directory` flag overrides it. Otherwise Git's `rcr` metadata directory is used, including for linked worktrees. A configured source-tree artifact directory must already be ignored.

Numeric changelog formats are `keep-a-changelog` and `intridea`. Optional `placeholder` and `sections` configure Keep a Changelog templates; Intridea accepts a placeholder, defaults to `- Your contribution here.`, and does not accept sections. Semantic mode uses `workflow = "semantic"`, defaults to generated notes and disabled next development, and accepts `changelog-format = "generated"` if explicitly supplied. Either mode accepts `changelog-format = "disabled"` to avoid all changelog file access while retaining numeric fallback or captured semantic release notes. Generated and disabled changelogs reject handwritten template settings.

`next-version` accepts `patch`, `disabled`, or an exact stable increasing SemVer. Explicit release overrides must also be stable; numeric releases cannot decrease the original manifest version. `github-repository = "owner/repository"` enables GitHub Release hosting with `GITHUB_TOKEN` or `GH_TOKEN`; omitting it skips hosting. Verification bounds are 1–60 attempts and 0–60 seconds between absence-only retries.

## Isolated Tests

The hidden `--test-registry DIRECTORY` option explicitly selects the filesystem publication adapter for shared source/installed executable fixtures. It never discovers production credentials or sends registry requests, is never automatically selected, and is not a production registry configuration option. Read-only verification requires that test directory to exist. `--test-artifact-api URL` additionally requires that adapter and accepts only a bare literal loopback origin with a fixed dummy token; it never uses production credentials. Real upload-protocol tests independently prove byte-for-byte HTTP publication at a loopback boundary.
