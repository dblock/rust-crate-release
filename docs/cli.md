# CLI and Configuration

## Commands

For automated releases, start with [README Usage](../README.md#usage); Actions installs rcr for you. This guide is for optional settings and local commands.

To use rcr locally, install it with Rust 1.99.1 or later:

```sh
cargo install --locked rust-crate-release --version 0.1.2
```

| Command | What It Does |
| --- | --- |
| `rcr release` | Prepares, publishes, pushes, and prepares the next development version in one invocation. |
| `rcr prepare` | Prepares locally without publishing or pushing. It still edits files, commits, and creates a tag: it is **not** a read-only dry run. |
| `rcr verify --name widget --release-version 0.1.1 --archive PATH` | Checks a published version against an archive without changing anything. |
| `rcr --version` | Prints the installed tool version. |

Use `--release-version` to select your crate's version, not the tool version. For a first publication with Cargo's saved credentials, follow [First-Time Publication](actions.md#publish-a-new-crate-once).

Run commands from your project root, or set `--root PATH`. Paths are relative to that root unless absolute. `--config FILE` selects a different configuration file; a missing explicitly selected file is an error. Without it, a missing `rcr.toml` is fine.

For scripts, successful commands print one JSON object to stdout; progress and errors go to stderr. Exit codes are 0 for success (including a bounded registry visibility timeout), 1 for an operational error, and 2 for invalid command syntax. See [Recovery](recovery.md#verify-without-publishing) for interpreting verification results.

## Local Publication

This is an advanced alternative to Actions. Start with a clean, up-to-date default-branch checkout and full Git history. Commit the intended version and `Cargo.lock`, and add meaningful notes if you keep a supported handwritten changelog. Configure Git identity and push authentication, and check that branch/tag rules permit release pushes.

Supply a crates.io token privately as `CARGO_REGISTRY_TOKEN`. Unlike Cargo, rcr does not read credentials saved by `cargo login`: it uploads the archive it checked rather than calling `cargo publish`. For a GitHub remote, also authenticate with `gh auth login` or set `github-release = false` to skip GitHub Releases.

```sh
rcr release
unset CARGO_REGISTRY_TOKEN
```

**This publishes and pushes a real release.** rcr runs formatting, tests, Clippy, and archive checks before publishing, then pushes the release and next-development changes. Remove the token even if the command fails. Never put it in Git, a command, or shell history. Check [partial-success recovery](recovery.md) before retrying.

### Inspect Before Publishing

Only split the commands if you want to inspect the prepared release first:

```sh
rcr prepare --save-inputs .git/rcr/source-inputs.json
# Inspect the archive, changelog, and local release commit/tag before continuing.
rcr release --inputs .git/rcr/source-inputs.json
```

The file carries the original source/version between these commands; it is not needed for plain `rcr release`. Artifacts and diagnostics are under Git's private metadata directory. Run `git rev-parse --git-path rcr` to find it, including in linked worktrees.

## Configuration

Create `rcr.toml` at your project root only if you want to change a default. For example, `next-version = "disabled"` stops rcr from advancing the manifest after publication.

No changelog is required. Numeric releases automatically detect a supported handwritten format in the configured changelog file; if it is absent, they skip file updates and use `Release X.Y.Z.` as notes. Unknown or mixed formats stop before source edits with guidance to select a format or disable updates.

| Setting | Use It To |
| --- | --- |
| `github-release = false` | Skip GitHub Release creation; still publish to crates.io and push the Git tag. |
| `next-version = "disabled"` | Leave the manifest at the released version. Select the next version yourself before releasing again. |
| `changelog-format = "automatic"` | Detect Keep a Changelog or Intridea, or skip a missing file; this is the numeric default. |
| `changelog-format = "keep-a-changelog"` | Require a specific handwritten format; `"intridea"` is also supported. |
| `changelog-format = "disabled"` | Leave any changelog file untouched, without detecting or validating its format. |
| `workflow = "semantic"` | Choose versions and notes from [commit messages](semantic.md). |
| `manifest = "crates/widget/Cargo.toml"` | Select a workspace member with an explicit version. |
| `default-branch = "main"` | Override automatic default-branch discovery. |
| `github-repository = "acme/widget"` | Override automatic GitHub repository discovery. |

Unknown settings, invalid values, and incompatible changelog/workflow choices are rejected. Validation commands are argument arrays, not shell scripts; use an explicit shell command if you need shell syntax.

### Default Settings

You do not need to copy these into a file. Set only the values you want to change:

```toml
workflow = "numeric"
manifest = "Cargo.toml"
remote = "origin"
timezone = "UTC"
changelog = "CHANGELOG.md"
changelog-format = "automatic"
next-version = "patch"
validation = [
  ["cargo", "fmt", "--all", "--", "--check"],
  ["cargo", "test", "--locked"],
  ["cargo", "clippy", "--locked", "--all-targets", "--", "-D", "warnings"],
]
verification-attempts = 6
verification-delay-seconds = 5
```

These are numeric defaults; branch and repository discovery need no TOML entries. `manifest` explicitly selects one package, including a workspace member. Changelog paths must stay inside the Git root without symbolic-link traversal. `artifact-directory` optionally selects storage; the global `--artifact-directory` flag overrides it. Otherwise Git's `rcr` metadata directory is used, including for linked worktrees. A configured source-tree artifact directory must already be ignored.

Numeric changelog updates default to `automatic`. Set `keep-a-changelog` or `intridea` to require one format explicitly. Automatic detection uses the configured `changelog` path and rejects empty, unknown, or mixed-format files. Optional `placeholder` and `sections` configure Keep a Changelog templates, including when detected automatically; Intridea accepts a placeholder, defaults to `- Your contribution here.`, and does not accept sections. Templates require an existing file in automatic mode. Semantic mode uses `workflow = "semantic"`, defaults to generated notes and disabled next development, and treats `automatic` as `generated`. Either mode accepts `changelog-format = "disabled"` to avoid all changelog file access while retaining numeric fallback or captured semantic release notes. Generated and disabled changelogs reject handwritten template settings.

`next-version` accepts `patch`, `disabled`, or an exact stable increasing SemVer. Explicit release overrides must also be stable; numeric releases cannot decrease the original manifest version. Verification bounds are 1–60 attempts and 0–60 seconds between absence-only retries.

### Repository Discovery

`default-branch` overrides discovery. In GitHub Actions, discovery uses `RCR_DEFAULT_BRANCH` supplied from the caller's repository metadata, then `repository.default_branch` in `GITHUB_EVENT_PATH`; otherwise it queries the configured remote's advertised HEAD. If it cannot find one, set `default-branch` explicitly. rcr never assumes the current branch is the default.

GitHub hosting uses `GITHUB_REPOSITORY` inside github.com Actions, otherwise the configured remote URL. HTTPS, SSH (`git@github.com:owner/name.git` and `ssh://`), and Git URLs are supported. Non-GitHub remotes skip hosting. `github-repository` overrides discovery; `github-release = false` disables hosting even with an explicit repository. GitHub authentication uses `GH_TOKEN`, `GITHUB_TOKEN`, or local `gh` credentials. Discovery does not configure the crates.io publisher or change repository protections.

## Project Runners

Use a project runner only when you need custom Rust hooks or providers. A binary can call `cli::main_with(|workflow| { /* register hooks/providers */ Ok(()) })` from `main`; library callers can use `cli::run_with(cli, &configure)`. This keeps stock parsing, configuration, JSON output, and Actions artifact verification.

Select it in `rcr.toml`:

```toml
runner = ["cargo", "run", "--locked", "--quiet", "--example", "release", "--"]
```

Alternatively, supply JSON argv through `RCR_RUNNER` or the Action's `runner` input; environment selection takes precedence. The CLI forwards arguments directly without a shell. The runner must print one JSON result and send progress to stderr. Call `main_with` or `run_with`, not ordinary `cli::run`, to avoid recursive dispatch. The configuration closure also runs during Actions preparation/publication.

Use the [custom hook implementations](releases.md#edit-a-file-in-a-hook) inside the configuration closure, obtaining the root from `workflow.components.package.root()`. Runners are trusted source code with release authority; do not remove artifact checks or weaken validators.

## Advanced Command Reference

To install a reviewed checkout instead of a published crate, use `cargo install --locked --path .`.

`prepare` and `release` accept `--release-version`, optional `--archive`, `--inputs`, and `--save-inputs`. An explicit nonempty archive wins over nonempty `CRATE_ARCHIVE`; empty values behave as unset. An invalid archive fails instead of silently rebuilding. Otherwise a matching retained archive is reused before building. Next-development validation removes inherited `CRATE_ARCHIVE`.

`verify` requires `--name`, `--release-version`, and `--archive`. It compares that explicit identity and archive with registry metadata and downloaded bytes. It needs no Git repository or current manifest, writes no receipts, and does not authenticate for publication. Release-only flags are rejected.

JSON key order is not guaranteed. Workflow phases and validation commands run in their configured order; repeated commands are not deduplicated.

## Captured Source Inputs

These are only needed when deliberately splitting preparation and publication; Actions manages them for you.

`--save-inputs` writes the original source/version to a private JSON file before preparation edits. It contains source identity/date, any explicit override, and semantic decisions, never credentials or workflow progress. Keep it under Git metadata or outside the checkout. Identical existing data is reusable; conflicting contents or symbolic-link destinations are rejected.

Pass the same file with `--inputs` for publication. rcr rechecks source ancestry, timestamp, identity, tags, and actual outputs; it does not restore progress from a receipt or reanalyze semantic commits against a newly created tag. A conflicting version override fails. Failed validation leaves edits for inspection and deliberate repair.

In Actions, inputs are bound to repository/workflow/run/attempt. A new run or attempt cannot automatically resume an old file. The hidden `--github-artifact-id` selects a stored upload whose metadata and bytes must match before publication; an ID or successful upload step alone is not proof.

The composite action calls `rcr actions guard`, `prepare`, and `release` internally. They require manual/default-branch hosted execution, use Git's rcr directory, and write outputs to `GITHUB_OUTPUT`. They honor `RCR_CONFIG`; preparation also reads `RCR_VERSION`, and release reads `RCR_ARTIFACT_ID` and the saved inputs. Use the supplied Actions integration rather than assembling these internal commands yourself.

## Isolated Tests

The hidden `--test-registry DIRECTORY` option explicitly selects the filesystem publication adapter for shared source/installed executable fixtures. It never discovers production credentials or sends registry requests, is never automatically selected, and is not a production registry configuration option. It suppresses production GitHub hosting and configured production publish commands; only an explicitly supplied `RCR_PUBLISH_COMMAND` can select an isolated command override in this mode. `--test-host DIRECTORY` additionally selects filesystem hosting. Read-only verification requires that test directory to exist. `--test-artifact-api URL` additionally requires that adapter and accepts only a bare literal loopback origin with a fixed dummy token; it never uses production credentials. Real upload-protocol tests independently prove byte-for-byte HTTP publication at a loopback boundary.
