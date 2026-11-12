<img src="assets/rust-crate-release.svg" alt="An open Cargo crate with an upward release arrow" width="160" height="160"> <!-- rumdl-disable-line MD033 MD041 -->
<!-- Logo-first layout and explicit image sizing are intentional. -->

# rust-crate-release

[![Crates.io](https://img.shields.io/crates/v/rust-crate-release.svg)](https://crates.io/crates/rust-crate-release) [![Test](https://github.com/dblock/rust-crate-release/actions/workflows/test.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/test.yml) [![Lint](https://github.com/dblock/rust-crate-release/actions/workflows/lint.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/lint.yml) [![Package](https://github.com/dblock/rust-crate-release/actions/workflows/package.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/package.yml)

Release your Rust crate from GitHub Actions. rcr tests your project, publishes to crates.io, optionally updates the changelog, creates a Git tag and GitHub Release, and prepares the next version. Ported from [ruby-gem-release](https://github.com/dblock/ruby-gem-release) and [npm-package-release](https://github.com/dblock/npm-package-release).

## Usage

You need a Cargo project on GitHub. No local rcr installation, `rcr.toml`, or changelog is required. Set up the workflow and trusted publisher once; after that, run the workflow when you are ready to release.

### 1. Add the Release Workflow

Add `.github/workflows/release.yml` to your Cargo project:

```yaml
name: Release

on:
  workflow_dispatch:

permissions:
  actions: read
  contents: write
  id-token: write

jobs:
  release:
    uses: dblock/rust-crate-release/.github/workflows/release.yml@v0.1.1
    with:
      tool-ref: v0.1.1
```

Commit this file and `Cargo.lock` to your default branch, even for a library. The workflow installs rcr and Rust for you.

### 2. First-Time Release

**Skip this step if your crate is already on crates.io.** crates.io requires the first publication to use an API token; trusted publishing works for subsequent versions.

Sign in to crates.io, verify your email, and create a short-lived [API token](https://crates.io/settings/tokens) with `publish-new` permission restricted to your crate name. Commit the intended `Cargo.toml` version and `Cargo.lock`. From your project root, authenticate interactively and publish:

```sh
cargo login
cargo publish --locked
cargo logout
```

Paste the token at Cargo's prompt, never into a command or GitHub secret. **This publishes your crate.** Confirm it on crates.io and revoke the token. Cargo does not increment the version: if you published `0.1.0`, change `Cargo.toml` to `0.1.1`, run `cargo check` to refresh `Cargo.lock`, and commit both before your first workflow release. See [First-Time Publication](docs/actions.md#publish-a-new-crate-once) for the full procedure.

### 3. Configure Trusted Publishing

In your GitHub repository's **Settings > Environments**, create `release` and allow deployments from your default branch. On crates.io, open your crate's **Settings > Trusted Publishing**, click **Add**, select **GitHub**, and enter:

| Field | Value |
| --- | --- |
| Repository owner | Your GitHub user or organization, for example `acme`. |
| Repository name | Your project's repository, for example `widget`. |
| Workflow filename | `release.yml` |
| Environment name (optional) | `release` |

Click **Add** to save. Enter `release` even though crates.io labels the environment optional. No Cargo-token secret or extra authentication action is needed.

The workflow must be allowed to push commits to your default branch and create `vX.Y.Z` tags. **A rule requiring pull requests blocks these pushes**, even with `contents: write`. Review [branch-protection requirements](docs/actions.md#configure-github) before releasing; publication can succeed before a blocked push fails.

### 4. Update Changelog

**Skip this step if you do not keep a changelog.** By default, rcr detects Keep a Changelog or Intridea format in `CHANGELOG.md`. If the file is absent, it skips file updates and uses `Release X.Y.Z.` as numeric release notes. No configuration is needed.

Add meaningful release notes to the `[Unreleased]` section of `CHANGELOG.md` before each release:

```markdown
# Changelog

## [Unreleased]

### Added

- Add widget serialization.
```

rcr detects this Keep a Changelog format, gives the notes a version and date, then creates a fresh `[Unreleased]` section for your next changes. Unknown or mixed formats stop with configuration guidance before release edits. To leave an existing changelog untouched, set `changelog-format = "disabled"` in `rcr.toml`. Other formats and generated notes are covered in [Changelogs](docs/changelogs.md).

### 5. Run a Release

Merge your changes onto the default branch, including release notes if changelog updates are enabled. On GitHub, select **Actions > Release > Run workflow**, choose the default branch, and click **Run workflow**.

Alternatively, dispatch with the GitHub CLI, replacing the repository and branch with yours:

```sh
gh workflow run release.yml --repo acme/widget --ref main
```

**This publishes the version in `Cargo.toml`.** For example, with `0.1.1` in the manifest, rcr publishes `0.1.1`, pushes tag `v0.1.1`, creates a GitHub Release, then commits and pushes `0.1.2` for development. To make a minor or major release instead, change the manifest version and refresh `Cargo.lock` before running the workflow.

Check the run's summary and your crate on crates.io. If a release fails, [check what completed before retrying](docs/recovery.md); a failed workflow may already have published the version.

### Advanced Usage

For custom before/after release behavior, including editing files, see [Custom Hooks](docs/releases.md#custom-hooks). Use [CLI and Configuration](docs/cli.md) for local commands and overrides, or [Semantic Releases](docs/semantic.md) to choose versions from commit messages.

## Configuration

`rcr.toml` is optional. rcr discovers your default branch and GitHub repository automatically.

Create `rcr.toml` only to override defaults. For example, to skip GitHub Release creation and next-development preparation:

```toml
github-release = false
next-version = "disabled"
```

See [CLI and Configuration](docs/cli.md#configuration) for all settings.

## Contributing

See [Contributing](CONTRIBUTING.md) for development and tests, [Design](DESIGN.md) for architecture, and [Changelog](CHANGELOG.md) for project changes.

## License

[MIT](https://github.com/dblock/rust-crate-release/blob/master/LICENSE).
