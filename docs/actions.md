# GitHub Actions

## Status and Prerequisites

The composite action and reusable workflow use the same rcr CLI and release engine as local runs. They require github.com GitHub-hosted runners, manual dispatch on the repository's actual default branch, a protected `release` environment, `contents: write`, `id-token: write`, full Git history, and non-canceling release concurrency. The composite cannot impose a caller job's environment, permissions, or concurrency; its caller must supply those prerequisites.

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

The action installs verified Rust 1.99.1 and Node 26.13.1, installs locked artifact support with scripts disabled, and builds the reviewed rcr source. Preparation saves immutable source inputs under Git metadata; release explicitly consumes that same repository/workflow/run/attempt-bound file. No checkpoint is downloaded or restored. Actions storage always uses Git's rcr metadata directory through the CLI override, avoiding checkout pollution and ensuring the final diagnostic step knows its location even on failure.

## Durable Preservation and Recovery

Before registry mutation, the Rust preservation provider invokes the reviewed Node helper and waits for the official `@actions/artifact` client to confirm a positive artifact ID and size. The helper validates the retained archive checksum, rejects symbolic links, freezes the selected archive/bundle/known diagnostic files in a private temporary directory, and uploads that snapshot. It does not recursively upload the checkout or accept an `uploaded=true` flag. Failed/missing/incomplete upload confirmation blocks publication.

Each preservation invocation creates a distinct immutable artifact snapshot; retries may preserve another copy but reconcile registry publication instead of duplicating it. Artifact-service quotas can therefore fail a repeatedly retried job before publication. Helpers never echo raw service failures or signed URLs. Registry masking commands use stderr so machine JSON on stdout remains valid.

The final diagnostic upload runs on success or failure, includes hidden files, and selects only crates, bundles, receipts, execution snapshots, and explicit source inputs. Bounded registry visibility timeout remains a successful but visibly unverified result; it does not block hosting or next-development preparation. Authentication, transport, metadata, or checksum failures remain errors. Inspect the retained bytes and actual registry/refs/hosted assets before deliberate recovery; fresh dispatches capture fresh source inputs rather than restoring an older attempt.

## Validation and Dependency Limits

Node tests exercise snapshot bytes, hidden storage, exact selection, awaited confirmation, service failure, missing files, changed checksums, symbolic links, and caller guards with an explicitly injected artifact client. Shared source/installed executable fixtures run the actual Actions wrapper and its real subprocess implementation against the real CLI/validators, explicitly injecting only the isolated publication adapter and simulated artifact helper. They also prove a failed uploader prevents publication and a successful helper runs before exact-byte publication. These tests never contact a live registry or artifact service. Actual OIDC and GitHub artifact-service behavior require a separately authorized release.

The current official artifact client 6.3.1 brings six moderate transitive npm advisories through archiver's glob/minimatch dependencies. The upload path uses literal `file`/`append` operations, and this helper supplies only validated filenames in an owned snapshot, not glob patterns or cancellation-driven traversal. Do not silently downgrade the current client or force incompatible dependency overrides; recheck upstream fixes when updating. This limitation is reported rather than presenting a clean audit or live-service validation.
