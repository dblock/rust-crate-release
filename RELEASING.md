# Releasing

## Current Status

This is an unpublished `0.1.0` library with typed workflow/changelog/semantic building blocks. No release CLI, registry publisher, release workflow, credentials, trusted publisher, or GitHub release environment is configured. Instructions below define acceptance criteria for the remaining implementation; they are not executable release commands or authorization to publish.

## Validate the Distribution

Run the local checks before committing or pushing release-related changes:

```sh
cargo test --locked --workspace
cargo xtask lint
cargo xtask package
```

The final command builds `target/package/rust-crate-release-0.1.0.crate`, verifies Cargo can compile it, checks archive paths, extracts that actual archive, and runs an isolated dependent program. It never publishes. Inspect the packaged manifest, lockfile, README, license, and changelog before any separately authorized release.

Check registry release status before changing versions. Keep unreleased `0.1.0` unchanged, and synchronize root Cargo version, committed lockfile, and Next changelog heading. Once released, follow [Versioning](AGENTS.md#versioning); release sections use `## <version> (YYYY/MM/DD)`, with newest versions first.

## First Publication and Authentication

The maintainer must separately authorize the first publication and complete authentication locally; never place tokens, challenge links, or codes in chat, commits, diagnostics, or repository secrets. [crates.io Trusted Publishing](https://crates.io/docs/trusted-publishing) currently requires the crate to have been published already; initial publication needs an API token. Recheck that requirement and current official tooling when implementing the release flow.

Cargo does not accept a prebuilt `.crate` in `cargo publish`. The planned publisher must upload the validated archive through Cargo's documented [registry upload protocol](https://doc.rust-lang.org/cargo/reference/registry-web-api.html#publish), with correctly normalized metadata and byte-for-byte evidence from simulated-registry tests. Do not substitute a source rebuild or assert exact-artifact publication from a successful dry run. The bootstrap publication procedure must be validated and documented after the publisher exists.

After a confirmed first release, separately authorize/configure a trusted publisher for the exact owner/repository, caller workflow filename, and environment. Use the official [crates.io authentication action](https://github.com/rust-lang/crates-io-auth-action) or a verified provider implementation to obtain short-lived OIDC credentials. Read back saved settings; configuration alone does not prove a successful authenticated publication. This scaffold neither configures a publisher nor requests a token.

## Future Automated Releases

Use manual dispatch, the default branch, a restricted `release` environment, explicit `contents: write`/`id-token: write`, concurrency control, and durable artifact upload before registry mutation. Define and test both reusable/composite interfaces against the same Rust engine. Do not copy the registry's tag-triggered quick-start example: release commit/tag creation belongs after archive publication in the agreed orchestration.

Any direct-push release metadata/next-version exception needs explicit setup authorization and verified protection changes. Retain active force-push/deletion protections with no bypass actors; document that removing a PR requirement would permit direct pushes by all writers, not just the release workflow. Ordinary development remains PR-based. No such exception is configured now.

After acceptance, push the matching commit/tag atomically, read back remote refs, publish the GitHub Release/archive, validate and push optional next-development edits, then verify registry SHA-256 against the retained archive using the explicit released version. Once the first registry release is verified, add a resolving crates.io version badge to README's existing badge block, linking to the actual published crate.

## Verify and Recover

Registry acceptance and index visibility are separate. Retry only structured version absence with bounded waits; visibility timeout records an unverified result, not a reason to republish. Authentication, transport, malformed metadata, identity, and integrity failures remain errors. Never infer publication failure solely from a timeout.

Recovery must inspect registry metadata/downloads, remote refs, hosted releases/assets, and retained archives/bundles before completing missing work. Matching outputs are reusable; conflicting bytes or tags fail closed. Diagnostic phase records do not authorize skipping checks, downloading/restoring execution state, moving tags, rewriting published versions, discarding source changes, or automatically retrying ambiguous uploads.

The future `rcr verify` must operate without publication or Git mutation and verify historical releases after the manifest advances. Retain private diagnostics without credentials and summarize unverified outcomes visibly. These requirements need source, installed-consumer, and simulated-registry tests before release tooling is considered implemented.
