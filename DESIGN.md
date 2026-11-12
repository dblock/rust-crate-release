# Design

## Scope and Status

Port [ruby-gem-release](https://github.com/dblock/ruby-gem-release/blob/master/DESIGN.md) and [npm-package-release](https://github.com/dblock/npm-package-release/blob/master/DESIGN.md) to Rust and Cargo. The first implementation covers their full existing feature set, not just numeric version bumps. The typed numeric/semantic release engine, rcr CLI/configuration, workflow/hook/execution building blocks, Markdown/changelogs, Cargo/registry/Git/hosting providers, durable diagnostics, and isolated testing helpers are implemented. Actions wrappers follow in a dependent PR.

Use repository/crate `rust-crate-release`, Rust import `rust_crate_release`, CLI `rcr`, and trusted built-in configuration `rcr.toml`. Package version starts at `0.1.0`. Rust 2024 and Rust 1.99.1 are the initial supported floor and development toolchain, verified against the stable distribution manifest. Runtime dependencies use public Markdown, SemVer, serde/JSON, error, TOML, archive, temporary-directory, SHA-256, HTTP/TLS, and glob APIs; verify current stable releases against this floor rather than copying the reference ecosystems.

## Public Interfaces and Layout

The library in `src/` will expose typed `Step<Input, Output>` contracts and sequential workflows whose adjacent types compose. Release lifecycle types encode validation, commitment, and artifact preparation prerequisites; they do not prove arbitrary custom providers safe. Keep immutable release identity separate from execution snapshots, platform clients, and diagnostics. Prefer Rust traits, structs, owned data, and explicit error types over a dynamic context map or literal translations of Ruby/JavaScript classes.

Built-in commands are `rcr prepare`, `rcr release`, and read-only `rcr verify`, with numeric and semantic workflows selected through data-only `rcr.toml`. Projects needing custom typed workflows, hooks, validators, or providers use the library from their own Rust runner, conventionally an `xtask`. Both interfaces use the same engine. [CLI and Configuration](docs/cli.md) defines commands, channels, options, compatibility errors, and defaults. Explicit immutable source-input files bridge separate commands; GitHub transport additionally binds repository/workflow/run/attempt, never downloaded execution state or diagnostic completion flags.

The root is one distributable library/CLI crate. The private `xtask/` workspace member provides maintenance commands and is excluded from the archive. `Cargo.lock` is committed for the executable. Shared fixtures in `tests/support/` run real processes against the source binary and the binary installed from the actual extracted archive, including unset/empty/supplied archive overrides, next-development validation, semantic decisions, historical verification, invalid configuration, and run-bound source inputs.

## Feature Parity

| Area | First Implementation Requirement |
| --- | --- |
| Composition | Typed steps, numeric and semantic workflows, observers, replaceable components, and a public Rust API. |
| Hooks | Ordered typed before/after hooks, trusted preparation/next edits, skipped-operation semantics, and failure context retaining the original error. |
| Providers | Independent package, registry, Git, hosting, changelog, version-policy, validator, artifact-store, verification, and local/GitHub execution interfaces. |
| Release Data | Immutable name/version/source identity, explicit manifest version, step history, outcomes versus conclusions, and credential-free execution snapshots. |
| Cargo | Root package discovery, manifest/lock synchronization, reproducible `.crate` packaging, installed-artifact checks, exact-byte upload, and SHA-256 verification. |
| Changelogs | Intridea and Keep a Changelog conventions, generated semantic notes, source-preserving Markdown section edits, Unicode/CRLF handling, and meaningful-note validation. |
| Semantic Releases | Conventional Commits, highest reachable stable tag, shallow-history rejection, source-bound analysis, explicit version overrides, and optional next-development preparation. |
| Git and Hosting | Clean/default-branch and concurrency guards, literal trusted staging, release commits/tags, atomic remote updates, recovery bundles, hosted notes, and archive assets. |
| Recovery | Independently idempotent output reconciliation, durable pre-publication artifacts, diagnostic receipts, standalone verification, and fail-closed conflicts. |
| Consumers | Disposable Git projects, local publication/hosting adapters, hook examples, and shared real-process source/installed CLI tests without live registry mutations. |
| Actions | A reusable workflow and composite action using the same engine, explicit permissions/environment/concurrency, durable artifact preservation, and failure summaries. |

Built-ins initially target one explicitly selected Cargo package, crates.io, Git, and optional GitHub Releases, matching the reference projects' single-package scope. Multi-crate dependency ordering, alternate built-in registries/hosts, prerelease channels, maintenance branches, and a general parallel scheduler are not existing peer features and remain outside this port. Public provider contracts must not hard-code GitHub as the execution platform or crates.io as every registry.

## Cargo Artifact Boundary

[`cargo package`](https://doc.rust-lang.org/cargo/commands/cargo-package.html) creates and verifies a `.crate`; [`cargo publish`](https://doc.rust-lang.org/cargo/commands/cargo-publish.html) packages again and accepts no retained archive argument. Therefore, invoking `cargo publish` and comparing its output afterward is not evidence that the checked bytes were uploaded.

The publisher uses Cargo's documented [registry Web API](https://doc.rust-lang.org/cargo/reference/registry-web-api.html#publish): a PUT to `/api/v1/crates/new` containing length-prefixed JSON upload metadata followed by the length-prefixed, retained `.crate` bytes. Derive and validate metadata from Cargo's normalized package manifest/metadata, including dependency renames, kinds, target conditions, features, registry mapping, license, README, and Rust version. Check lengths, identity, publish restrictions, and credential origin before mutation. Keep the package builder and registry uploader independent.

Prove this boundary against a local simulated Cargo-compatible registry: capture the request, compare its archive bytes and SHA-256 to the validated artifact, inspect metadata for supported manifest forms, then build consumers from the downloaded archive. A successful Cargo dry run alone is insufficient. No real-registry upload is part of scaffold or PR tests.

## Release Sequence and Recovery

Capture source identity/date once; reject dirty/non-default/shallow or conflicting state before edits. Stage only trusted preparation changes and retain the intended tree. Validate, commit, package reproducibly, inspect the normalized manifest, exercise the actual archive, and create the local tag/bundle. Preserve artifacts durably before any publication; Actions upload failure must prevent upload to the registry.

`prepare` stops before publication and whole-release success. `release` repeats preparation and reconciles each actual output; no downloaded checkpoint, phase flag, or diagnostic journal may authorize skipping work. Reuse matching immutable artifacts, registry checksums, tags, commits, and hosted assets; fail on conflicts without moving published tags, discarding changes, or automatically republishing ambiguous uploads.

After registry acceptance, atomically push the release commit/tag, read back refs, create the hosted release with the archive, then validate/commit/push the optional next development version. Final verification uses the captured released version and retained archive, never the advanced manifest or HEAD. Index visibility is separate from upload acceptance: bound retries for structured absence only, retain an unverified outcome on visibility timeout, and fail on authentication, transport, metadata, or checksum errors. Persist diagnostics and summaries without credentials.

Numeric workflows use handwritten meaningful changelogs and a default next patch. Semantic workflows capture Conventional Commits analysis once against the source and highest reachable numeric tag; first release defaults to `1.0.0`, breaking commits imply major even in `0.x`, and next-development edits default off. Generated notes and source-bound decisions must survive separate Actions preparation/publication commands without reanalyzing against a newly created tag.

## Authentication and Actions

The registry owns authentication through the execution provider; YAML must not embed ecosystem-specific publishing logic. [crates.io Trusted Publishing](https://crates.io/docs/trusted-publishing) currently requires an already published crate and exchanges a GitHub OIDC identity for a short-lived token. The official [`rust-lang/crates-io-auth-action`](https://github.com/rust-lang/crates-io-auth-action) documents that exchange. Do not copy npm token semantics, assume pending-publisher support, or persist tokens in diagnostics, Git, or repository secrets.

The future reusable workflow will require manual/default-branch execution, a `release` environment, explicit OIDC/content permissions, durable artifacts, and concurrency guards. Composite callers supply equivalent prerequisites. Initial publication, authentication, trusted-publisher setup, release workflows, protection changes, dispatch, tags, and merges need separate authorization; the scaffold configures none of them.

## Validation and Maintenance

Separate Test, Lint, and Package workflows exercise Rust's supported floor and current stable across Linux, macOS, and Windows using explicit `matrix.entry` lists. `cargo xtask lint` combines rustfmt, Clippy, rustdoc warnings, rumdl Markdown structure checks, typos spelling, actionlint, and tested Markdown/YAML-aware policy checks. `cargo xtask package` builds, inspects, extracts, and compiles a consumer against the actual `.crate`, not the source checkout. Distribution validation does not perform registry publication; loopback runtime tests prove the upload boundary separately.

Exercise real workflows against isolated Git remotes and local publication boundaries, including hook failures, concurrent/failed pushes, delayed visibility, ambiguous upload recovery, and verification after the manifest advances. Include subprocess coverage and meaningful error paths; aim near 100% of project-owned runtime code without trivial tests or excluding real logic. Coverage includes runtime and maintenance code; production authentication is deliberately not exercised, and unavailable branch instrumentation is reported rather than inferred.

Keep this design focused on the current agreed architecture and relevant constraints; remove superseded proposals rather than appending an implementation diary.
