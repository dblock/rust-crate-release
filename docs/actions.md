# GitHub Actions

## Status and Prerequisites

The composite action and reusable workflow use the same rcr CLI and release engine as local runs. They require github.com GitHub-hosted runners, manual dispatch on the repository's actual default branch, a protected `release` environment, `actions: read`, `contents: write`, `id-token: write`, full Git history, and non-canceling release concurrency. The composite cannot impose a caller job's environment, permissions, or concurrency; its caller must supply those prerequisites.

The crate/tool is unpublished. Examples below show the intended `v0.1.0` interface, not an existing tag. Before that release, reviewers must pin both reusable-workflow and `tool-ref` references to the same reviewed implementation commit. No self-release dispatch, publisher, credentials, environment, or protection exception is configured by this project.

Initial crates.io publication needs separate authorization and a private API token in a local run. Configure crates.io Trusted Publishing only after the crate exists, for the exact repository/caller workflow/environment. Automated releases reject a stored `CARGO_REGISTRY_TOKEN` and obtain short-lived OIDC credentials through the registry provider. Never store registry tokens in repository secrets. Source protections must separately permit the authorized atomic release and next-development pushes; see [Releasing](../RELEASING.md).

## Reusable Workflow

A consumer `.github/workflows/release.yml` can contain:

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

The reusable workflow checks out the consumer and reviewed tooling into sibling directories, not an untracked tooling folder inside the consumer. It configures a local bot commit identity, uses the `release` environment and non-canceling repository/ref concurrency, then calls the same composite action. Pin `tool-ref` to the same reviewed ref as `uses`; it makes the tooling checkout explicit rather than silently running the latest default branch. The workflow exposes released `version` and JSON `verification` outputs.

Configure `rcr.toml` in the consumer, including its actual `default-branch` if not `master`. Optional `config` and `version` workflow inputs select the trusted configuration file and explicit release version. Use `github-repository = "owner/repository"` to enable GitHub Release hosting; otherwise that phase is skipped.

## Composite Action

Custom caller jobs may use `dblock/rust-crate-release@v0.1.0` with `root`, optional `config`, and optional `version` inputs. First check out full history, configure a local Git author, and supply the job's `release` environment, permissions, default-branch dispatch guard, and non-canceling concurrency. Outputs are `name`, `version`, `archive`, `checksum`, and JSON `verification`.

The action installs verified Rust 1.99.1 and builds the reviewed rcr source. All project-owned orchestration, verification, fixtures, and tooling are Rust; there is no Node/npm setup, JavaScript helper, or npm lockfile. Standard third-party actions such as checkout and upload-artifact still use GitHub's managed runtimes. Preparation saves immutable source inputs under Git metadata; release explicitly consumes that same repository/workflow/run/attempt-bound file. No checkpoint is restored or used to resume phases. Actions storage always uses Git's rcr metadata directory through the CLI override, avoiding checkout pollution and ensuring the final diagnostic step knows its location even on failure.

## Durable Preservation and Recovery

The Rust preparation command completes validation, packaging, and bundle creation without publication. A separate `actions/upload-artifact` step uploads only the exact crate, recovery bundle, immutable source inputs, and known diagnostics as `rcr-release-<run>-<attempt>`. This standard action has the artifact-service credentials that GitHub does not expose to composite shell steps; no private upload protocol is reimplemented.

Before registry mutation, the Rust preservation provider independently reads that artifact's metadata through GitHub's public [artifact REST API](https://docs.github.com/en/rest/actions/artifacts), verifies its ID/name/run/attempt/non-expired state, and downloads the ZIP. It checks the stored size and optional SHA-256 digest, rejects unexpected paths and non-regular entries, and compares the crate, bundle, and source-input files byte-for-byte with retained regular files. Required files must be nonempty; downloaded diagnostics are bounded and never restored. ZIP downloads are bounded to 1 GiB and individual diagnostics to 2 MiB. Missing artifacts, permissions, files, service failures, or mismatching bytes block publication. A successful upload step or supplied ID alone is insufficient.

Artifact names are immutable within a run/attempt; a conflicting existing name fails instead of overwriting recovery evidence. A new workflow attempt uses a new name and fresh bound inputs. Deliberate same-attempt CLI retries can reverify the existing immutable artifact while reconciling actual publication outputs. Artifact-service quotas can fail a retried job before publication. Repository tokens are sent only to the API, never to signed download redirects; raw service bodies, tokens, and signed URLs are not echoed. Registry masking commands use stderr so machine JSON on stdout remains valid.

The final diagnostic upload runs on success or failure, includes hidden files, and selects only crates, bundles, receipts, execution snapshots, and explicit source inputs. Bounded registry visibility timeout remains a successful but visibly unverified result; it does not block hosting or next-development preparation. Authentication, transport, metadata, or checksum failures remain errors. Inspect the retained bytes and actual registry/refs/hosted assets before deliberate recovery; fresh dispatches capture fresh source inputs rather than restoring an older attempt.

## Validation and Service Limits

Rust unit tests exercise downloaded bytes, required files, unexpected paths, symbolic links, caller guards, metadata/run/attempt conflicts, size limits, failed requests, and token-free signed redirects. Shared source/installed executable fixtures run the actual Rust Actions commands and real CLI/validators against isolated publication and a loopback artifact REST service. A tampered remote crate prevents publication; restoring the immutable snapshot permits exact-byte publication. These tests never contact a live registry or artifact service.

Actual OIDC, standard-action uploads, permission enforcement, and GitHub artifact-service behavior require a separately authorized release; local protocol fixtures do not prove live integration. Artifact storage must remain available until publication completes. The project no longer maintains an npm dependency tree; the standard upload action remains an upstream dependency with its own implementation and update policy.
