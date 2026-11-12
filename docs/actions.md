# GitHub Actions and Trusted Publishing

Use rust-crate-release `v0.1.0` to release your Cargo project from GitHub Actions without storing a crates.io token. This guide uses crate `widget`, repository `acme/widget`, and default branch `main`; replace them with your own values. The crate name, repository name, and workflow filename are separate identities.

## Prepare Your Project

Release one explicitly selected Cargo package with an explicit stable version in `Cargo.toml`. Commit a reviewed `Cargo.lock`, even for a library; generate it with `cargo generate-lockfile` if it is missing. A virtual workspace is not an implicit package selection: use the `manifest` option for a workspace member. Workspace-inherited versions and coordinated multi-crate releases are not supported.

Add `rcr.toml` at the repository root:

```toml
default-branch = "main"
github-repository = "acme/widget"
changelog-format = "keep-a-changelog"
next-version = "patch"
```

Set `default-branch` to the actual default branch, not necessarily `main`; the default when omitted is `master`. `github-repository` enables GitHub Release creation and attaches the exact crate archive; omit it to publish only to crates.io and Git. The manifest version is the next numeric release, and the default next-development policy increments its patch. For example, releasing `0.1.1` leaves `0.1.2` in the development manifest and lockfile.

Create `CHANGELOG.md` with meaningful unreleased notes:

```markdown
# Changelog

## [Unreleased]

### Added

- Add widget serialization.
```

Use `changelog-format = "intridea"` for a numeric `X.Y.Z (Next)` section with `- Your contribution here.`, or choose [semantic releases](cli.md#configuration) to generate notes from Conventional Commits. Do not dispatch a numeric release with an empty changelog. Commit the configuration, lockfile, notes, and workflow through your normal review process before releasing.

Default validation runs Cargo formatting, locked tests, and Clippy. Packaging and actual-archive consumers run offline after validation, so dependencies must already be cached; normal test/Clippy builds populate that cache. Custom validators must also restore dependencies and provide any additional tools they need. The [composite action](#custom-jobs-with-the-composite-action) lets you add project-specific setup steps.

## Publish a New Crate Once

[crates.io Trusted Publishing](https://crates.io/docs/trusted-publishing) requires an existing published crate and a crates.io owner account. If your crate already exists, skip to GitHub setup; never republish an existing version.

For a new crate, publish its first version locally with rcr and a narrowly scoped crates.io API token. Install `cargo install --locked rust-crate-release --version 0.1.0`, ensure your Git identity and remote-push authentication work, and follow [Local Publication](cli.md#local-publication). Supply `CARGO_REGISTRY_TOKEN` privately only for that local release, then remove it from the environment. If GitHub hosting is enabled, authenticate GitHub separately through `gh auth login` or a private `GH_TOKEN`.

The local release publishes the checked crate, pushes its release commit/tag, and prepares the next development version. Confirm the version on crates.io and the matching `vX.Y.Z` Git tag before configuring trusted publishing. Do not put the bootstrap token in GitHub secrets; automated releases below reject `CARGO_REGISTRY_TOKEN`.

## Configure GitHub

Create a GitHub environment named `release` under your consumer repository's **Settings > Environments**. Restrict its deployment branches to your actual default branch, such as `main`, and add required reviewers if desired. The reusable workflow already selects this environment; you do not add `environment` to the caller job that uses it.

Allow the workflow's ordinary, non-force pushes to the default branch and creation of `vX.Y.Z` tags. rcr publishes the crate, then atomically pushes a release commit/tag and later a next-development commit; it does not open version-update PRs. `contents: write` does not bypass branch rules.

In a no-bypass ruleset setup, a **Require a pull request before merging** rule on the default branch blocks these pushes and must be removed after reviewing that tradeoff. This permits direct pushes by all writers, not just the workflow; keep ordinary development PR-based. Retain active force-push and branch-deletion protections. Required-status, signing, restricted-push, and tag rules must also permit the generated commits and new tags; otherwise publication can succeed before a Git push fails. Do not weaken rules automatically or assume a successful crate upload proves Git completed.

Ensure organization policies allow this reusable workflow and its actions. No repository Cargo-token secret, personal access token, or `secrets: inherit` setting is needed for the standard setup.

## Configure the crates.io Publisher

As a crate owner, open your crate's **Settings > Trusted Publishing**, click **Add**, select GitHub, and save these fields:

| Field | Example Value | Meaning |
| --- | --- | --- |
| Repository owner | `acme` | Your GitHub user or organization. |
| Repository name | `widget` | Your consumer repository, not `rust-crate-release`. |
| Workflow filename | `release.yml` | Your caller file's basename, not its full path. |
| Environment | `release` | The GitHub environment configured above. |

Use your caller repository and workflow even when it invokes the reusable workflow in `dblock/rust-crate-release`. Keep the filename and environment in the saved publisher consistent with the YAML below. Save a separate configuration for each crate you intend to publish; this tool releases only the selected package.

rcr requests a GitHub OIDC identity with audience `crates.io`, exchanges it for a short-lived publish token, and revokes that token after the upload attempt. You do not add `rust-lang/crates-io-auth-action`, call `cargo publish`, or pass a token into this workflow. The [official crates.io authentication action](https://github.com/rust-lang/crates-io-auth-action) documents the same exchange, but rcr performs it internally.

## Add the Release Workflow

Create `.github/workflows/release.yml` in your consumer repository:

```yaml
name: Release

on:
  workflow_dispatch:
    inputs:
      version:
        description: Optional explicit stable version
        required: false
        default: ''
        type: string

permissions:
  actions: read
  contents: write
  id-token: write

jobs:
  release:
    uses: dblock/rust-crate-release/.github/workflows/release.yml@v0.1.0
    with:
      tool-ref: v0.1.0
      version: ${{ inputs.version }}
```

Both refs select the tool's release, not your crate's version. Keep `uses` and `tool-ref` pinned to the same reviewed tag or commit. The optional `version` input selects your crate's release version; when empty, numeric releases use the manifest version. An optional `config` input selects another configuration file relative to your project root.

The reusable workflow supplies an Ubuntu GitHub-hosted runner, full-history consumer checkout, sibling tooling checkout, bot commit identity, the `release` environment, and non-canceling repository/ref concurrency. It runs only for manual dispatch on the actual default branch. It exposes `version` and JSON `verification` outputs.

| Permission | Purpose |
| --- | --- |
| `actions: read` | Independently download and verify the durable recovery artifact before publication. |
| `contents: write` | Push release/development commits and tags, and create GitHub Releases when enabled. |
| `id-token: write` | Request the OIDC identity accepted by the crates.io publisher. |

Merge the caller workflow and configuration onto the default branch. Do not use a tag-push trigger: rcr creates the release tag after crate publication.

## Run and Verify a Release

Add meaningful notes and commit any intended manifest-version change. Ensure the default branch is clean, up to date, and has no competing release. Then select **Actions > Release > Run workflow**, choose the default branch, and leave `version` empty unless deliberately overriding the manifest version.

The equivalent GitHub CLI command is:

```sh
gh workflow run release.yml --repo acme/widget --ref main
```

To explicitly select a higher stable release version, use `-f version=0.2.0`. Dispatch is a real release, not a dry run: it can publish, create tags/releases, and push development commits. To check locally without publication, use `rcr prepare`; that still edits, commits, and tags locally.

Inspect the run's steps and job summary. The order is validation/preparation, exact-archive packaging, durable artifact upload and independent verification, crates.io publication, atomic release push, optional GitHub hosting, next-development validation/push, and explicit-version registry verification. Confirm the released version on crates.io, its matching Git tag, GitHub Release when enabled, and the resulting development version.

Recovery files appear as `rcr-release-<run-id>-<attempt>` and final diagnostics as `rcr-diagnostics-<run-id>-<attempt>`. To download the immutable pre-publication snapshot outside your checkout and verify a release independently, replace the example IDs/version:

```sh
gh run download RUN_ID --repo acme/widget \
  --name rcr-release-RUN_ID-RUN_ATTEMPT --dir ../widget-release-artifacts
rcr verify --name widget --release-version 0.1.1 \
  --archive ../widget-release-artifacts/widget-0.1.1.crate
```

Registry visibility can lag upload acceptance. A `not_visible` verification result is a successful but unverified release outcome; it does not prevent hosting/development or authorize republishing. Run the read-only verification command later. Authentication, transport, metadata, or checksum errors remain failures. See [Diagnostics and Recovery](recovery.md) before retrying partial success; do not blindly dispatch again after a failed push or hosting step.

## Custom Jobs with the Composite Action

Use `dblock/rust-crate-release@v0.1.0` for project-specific dependency/tool setup. Unlike the reusable workflow, a composite cannot impose job-level environment, permissions, concurrency, or checkout policy. Supply them explicitly:

```yaml
name: Release

on:
  workflow_dispatch:

permissions:
  actions: read
  contents: write
  id-token: write

concurrency:
  group: rcr-${{ github.repository }}-${{ github.ref }}
  cancel-in-progress: false

jobs:
  release:
    name: Release Crate
    if: github.ref == format('refs/heads/{0}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    environment: release
    steps:
      - name: Check Out Source
        uses: actions/checkout@v7.0.1
        with:
          fetch-depth: 0
      - name: Configure Release Identity
        run: |
          git config user.name "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
      - name: Release Crate
        uses: dblock/rust-crate-release@v0.1.0
```

Insert your additional setup before the final step. The action accepts `root` (default `.`), `config`, and `version`; outputs are `name`, `version`, `archive`, `checksum`, and JSON `verification`. Keep the selected Cargo package inside its Git root and do not leave untracked tooling inside the checkout.

The action installs Rust 1.99.1 and builds the reviewed rcr source. Orchestration, verification, fixtures, and tooling are Rust. Your validators must remain compatible with the selected toolchain; use your own Rust runner when you need custom typed composition.

## Preservation and Failure Boundaries

The Rust preparation command saves immutable source inputs under Git metadata and completes validation, packaging, and bundle creation without publication. A separate standard upload action preserves only the exact crate, recovery bundle, source inputs, and known diagnostics. This action receives artifact-service credentials that GitHub does not expose to composite shell steps; rcr does not reimplement that private upload protocol.

Before registry mutation, Rust reads the immutable artifact through GitHub's public [artifact REST API](https://docs.github.com/en/rest/actions/artifacts). It checks ID/name/run/attempt/expiry, stored size and optional SHA-256 digest, ZIP entries, and exact retained crate/bundle/source-input bytes. Missing, non-regular, or mismatching required files block publication. ZIP downloads are bounded to 1 GiB and individual diagnostics to 2 MiB. A supplied artifact ID or successful step alone is not proof of preservation.

Names are immutable within a run/attempt; conflicts fail rather than overwrite recovery evidence. A new attempt captures fresh bound inputs instead of restoring an older attempt's state. Deliberate same-attempt retries reverify the snapshot and reconcile actual outputs. Downloaded diagnostics are never interpreted as skip instructions. Repository tokens never follow signed download redirects, and raw tokens, signed URLs, and service bodies are not echoed.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Workflow is skipped or the caller guard fails | Dispatch the caller workflow on the actual default branch, on a GitHub-hosted runner. |
| OIDC token exchange fails | Confirm crate ownership/existence, the saved consumer repository/workflow/environment, and `id-token: write`. |
| Stored-token guard fails | Remove `CARGO_REGISTRY_TOKEN` from job/environment settings and any separate auth-action step. |
| Artifact verification fails | Confirm `actions: read`, successful upload, storage availability, and unchanged retained files; do not bypass the check. |
| Git push fails after publication | Reconcile crates.io and remote refs first; inspect PR/status/signing/tag rules before deliberate recovery. |
| Validation or offline packaging fails | Review the real validator output, committed lockfile, dependency cache, and required project tools. |
| Verification says `not_visible` | Wait, then verify the explicit released version/archive; do not upload again. |

Rust tests cover real source/installed commands, isolated Git remotes, simulated publication, and a loopback artifact service. They do not establish live OIDC, GitHub storage behavior, or your repository's permission/protection configuration. A successful real release is the end-to-end confirmation of that setup.
