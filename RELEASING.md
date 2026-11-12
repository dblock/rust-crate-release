# Releasing

To release your own Cargo project with rust-crate-release, follow [GitHub Actions and Trusted Publishing](docs/actions.md). This document covers maintenance releases of rust-crate-release itself. Release instructions are not permission to publish, dispatch workflows, create tags, configure credentials, or change protections.

## Choose the Version

Check the current manifest version against crates.io before changing it. Keep an unreleased version unchanged; once it is published, follow [Versioning](AGENTS.md#versioning) and synchronize `Cargo.toml`, `Cargo.lock`, and the Next changelog heading. Do not infer publication from examples in the documentation, a prepared archive, or a Git tag alone.

Keep meaningful, attributed changes under the current `## X.Y.Z (Next)` section of `CHANGELOG.md`, with the contribution placeholder last. rcr dates the release section during preparation and creates the next patch development section afterward. Never overwrite an existing registry version or move a published release tag.

## Validate the Distribution

Run local checks on the exact revision before committing or pushing release-related changes:

```sh
cargo test --locked --workspace
cargo xtask lint
cargo xtask package
```

Package validation builds `target/package/rust-crate-release-<version>.crate`, checks its paths and allowlist, verifies Cargo compilation, and runs an extracted-library consumer and the installed executable's shared real-process fixtures. It does not publish. Inspect the archive's normalized manifest, lockfile, README, license, changelog, and shipped action/workflow before release.

The root `rcr.toml` selects this repository's Intridea changelog, GitHub hosting, workspace tests, full lint, real package/consumer validation, and source-owned `rcr-release` runner. Those commands also run during next-development validation. The runner updates README and usage-document installation/action pins to the released version, not the next manifest version, in the validated next-development commit after publication. It leaves consuming-crate example versions unchanged and does not invent an unreleased tag. This maintenance configuration and runner are excluded from the distributed crate; consumers provide their own configuration.

## Publish Locally

Use a clean, full-history default-branch checkout matching the remote. Configure the Git author and remote-push authentication, and build the reviewed tool:

```sh
cargo build --locked --bin rcr
./target/debug/rcr prepare --save-inputs .git/rcr/source-inputs.json
```

Preparation validates, commits, packages, creates the local release tag, and retains recovery files without publishing or pushing. Inspect the prepared archive, notes, commit/tag, and diagnostic identity before proceeding. Cargo cannot publish a supplied `.crate`; rcr uses the documented [registry upload protocol](https://doc.rust-lang.org/cargo/reference/registry-web-api.html#publish) to publish those exact checked bytes rather than repackaging.

Supply an authorized, narrowly scoped crates.io API token privately as `CARGO_REGISTRY_TOKEN`; this is required for a crate's first publication. Provide GitHub authentication separately through `gh auth login` or a private `GH_TOKEN`/`GITHUB_TOKEN`. Do not put credentials in chat, shell history, Git, diagnostics, or repository secrets.

After confirming that branch/tag protections permit the release and next-development pushes, publish:

```sh
./target/debug/rcr release --inputs .git/rcr/source-inputs.json
unset CARGO_REGISTRY_TOKEN
```

Remove the token even after a failure, and reconcile actual outputs before retrying. Publication, Git pushes, hosting, next-development preparation, and verification are separate operations; a failed later step does not undo an accepted crate upload.

## Configure Trusted Publishing

After the crate exists on crates.io, follow the [consumer setup guide](docs/actions.md) for the `release` environment, compatible branch/tag rules, and a per-crate trusted publisher. For this repository, publisher identity uses owner `dblock`, repository `rust-crate-release`, caller workflow `publish.yml`, and environment `release`. The manual [Publish workflow](.github/workflows/publish.yml) invokes the local reusable release workflow with `tool-ref: github.sha`, so the tool and project runner come from the reviewed dispatched commit rather than a moving external ref. Only dispatch it separately after authorizing a real release and confirming the external settings.

The caller needs `actions: read`, `contents: write`, and `id-token: write`. rcr performs the OIDC exchange and short-lived token cleanup itself; do not add a separate authentication action or set `CARGO_REGISTRY_TOKEN` in the automated job. Pin the caller's reusable-workflow and `tool-ref` references together.

Review protection changes separately. Release automation needs direct pushes of release metadata and the next development version, not PRs for those commits. Removing a PR requirement permits direct pushes by all writers, not just automation; ordinary development remains PR-based. Retain active force-push and deletion protections with no bypass actors.

Configuration alone does not prove a trusted release works. Read back the saved publisher/environment/permissions and confirm them through a separately authorized real release. The manual caller and runner do not configure external settings or bypass protections, and must not be dispatched merely to test these instructions.

## Confirm and Recover

Check crates.io for the explicit released version and checksum, confirm the matching remote `vX.Y.Z` tag/commit and hosted archive, then inspect the next development manifest, lockfile, changelog, and remote branch. Verification always uses the released version and retained archive, never the now-advanced manifest or HEAD.

Registry visibility can lag acceptance. Bound retries only for structured version absence; a visibility timeout records an unverified outcome without blocking hosting/development or authorizing another upload. Authentication, transport, metadata, identity, and checksum errors remain failures. Use `rcr verify --name rust-crate-release --release-version X.Y.Z --archive PATH_TO_RELEASED_CRATE`, replacing the version and path with the retained release.

Follow [Diagnostics and Recovery](docs/recovery.md) before completing missing operations. Never restore a diagnostic completion flag as authority, discard unexpected source edits, move a tag, or assume a failed response means publication did not happen.

After confirming a registry release, verify that the version's installation command and matching `vX.Y.Z` workflow refs resolve, and add or verify the crates.io version badge in README. Release date, version metadata, and registry badges must reflect actual publication, not release-ready documentation.
