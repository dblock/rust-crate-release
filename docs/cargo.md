# Cargo Packages and Registries

The package and registry providers release one explicitly selected Cargo package and publish its retained `.crate` bytes. They do not invoke `cargo publish`, coordinate multiple package versions, update registry configuration, or authorize a release.

## Package Selection

`package::CargoPackage::new(root)` selects `root/Cargo.toml`; `CargoPackage::from_manifest(manifest)` explicitly selects a package manifest, including a workspace member. A virtual workspace is not an implicit package selection. Both constructors return `Result<CargoPackage>`. Import `package::PackageProject` to use the provider methods.

```rust
use rust_crate_release::package::{CargoPackage, PackageProject};
use semver::Version;
use std::path::Path;

fn prepare(root: &Path, artifacts: &Path, timestamp: i64) -> rust_crate_release::Result {
    let package = CargoPackage::new(root)?;
    let metadata = package.metadata()?;
    package.set_version(&Version::new(1, 2, 3))?;
    let artifact = package.build(artifacts, timestamp)?;
    package.verify_consumer(&artifact)?;
    assert_eq!(artifact.name(), metadata.name);
    artifact.verify()
}
```

`PackageProject: Send + Sync` exposes `root() -> &Path`, `metadata() -> Result<PackageMetadata>`, `set_version(&Version) -> Result`, `build(&Path, i64) -> Result<Artifact>`, and `verify_consumer(&Artifact) -> Result`. `PackageMetadata` has public `name: String`, `version: semver::Version`, and `manifest_path: PathBuf` fields.

## Version and Lockfile Changes

Version updates preserve unrelated TOML, comments, and uniform LF or CRLF newlines. The selected package must have an explicit local version; workspace-inherited versions fail rather than editing a shared workspace value. Mixed newline styles are rejected rather than silently rewritten. The matching local package entry in the workspace `Cargo.lock` and its version-qualified local dependency references are synchronized without resolving or updating unrelated dependencies.

An existing, reviewed `Cargo.lock` is required for version updates and packaging. Generate or restore it explicitly before release preparation; the provider never generates a missing project lockfile or refreshes dependencies as a side effect of a version change. Failed external commands do not reset Git, discard user changes, or roll back earlier edits.

## Retained Archives

`build(directory, source_date_epoch)` invokes `cargo package --locked --offline --allow-dirty` with the supplied nonnegative timestamp as `SOURCE_DATE_EPOCH`. Cargo performs its normal extracted-package compilation. Dependencies must already be available in the local Cargo cache; dependency restoration is an explicit prerequisite. The release engine owns the clean-tree boundary, not `--allow-dirty`.

The generated archive is inspected before its bytes are retained as `directory/name-version.crate`. An existing destination is reused only when its bytes are identical; a conflict is never overwritten. The artifact owns its path, name, version, SHA-256 checksum, and registry upload metadata through private fields with `path()`, `name()`, `version()`, `checksum()`, and `upload_metadata()` getters.

`Artifact::open(&Path) -> Result<Artifact>` reads a real `.crate`, rejects malformed gzip data, unsafe paths, links, duplicate entries, and multiple package roots, and extracts using the public tar API. The normalized manifest and `cargo metadata --locked --offline --no-deps --format-version 1` establish identity and upload metadata; receipt fields and the current checkout are not trusted. Package roots, target sources, README files, license files, and dependency paths must remain inside the archive boundary. Packages that prohibit crates.io publication are rejected.

Upload metadata preserves dependency renames, feature selection, optional and default-feature flags, normal/build/development kinds, target conditions, registry indexes, package features, badges, and Rust version. The README body and relative path are included in metadata. A license-file path is included as required by the Cargo registry protocol, and its contents are validated from the retained archive rather than invented as an additional protocol field.

`Artifact::verify() -> Result` rereads the retained bytes and rechecks the checksum, archive identity, and metadata. `verify_consumer` compiles the archive's actual library and binary targets offline; Rust-linkable libraries are also compiled through an isolated path-dependency consumer. It never substitutes the workspace source or executes an arbitrary binary. Validation therefore remains bound to the released artifact after source files or the development version change. Owned scratch directories use the standard OS temporary directory and are removed after each inspection or consumer check. If that directory is inside a Cargo project, validation fails clearly rather than inheriting an unrelated workspace. Extraction does not alter the normalized manifest or add a workspace marker.

`Artifact::vcs_commit(&self) -> Result<Option<String>>` reads `.cargo_vcs_info.json` from verified retained bytes. It returns `None` when the file is absent, otherwise validates Cargo's `git.sha1` as a complete 40- or 64-digit hexadecimal commit and returns it in lowercase. Malformed metadata, an invalid `git.dirty` field, or `git.dirty = true` fails this explicit source-identity check. Opening or generally verifying an archive still permits dirty packaging for development validation. Before registry upload, the release engine should require the archive commit to equal the captured prepared release commit, including for supplied archive overrides; matching only name and version does not establish source identity.

## Registry Publication

`registry::PackagePublisher: Send + Sync` exposes `lookup(&str, &Version) -> Result<Option<PublishedCrate>>`, `publish(&Artifact, &dyn ExecutionProvider) -> Result<PublishOutcome>`, and `download(&str, &Version) -> Result<Vec<u8>>`. `PublishedCrate` has public `name: String`, `version: Version`, and `checksum: String` fields. `PublishOutcome` is `Published` or `Reused`.

`CratesIoPublisher::new() -> Result<CratesIoPublisher>` targets the official HTTPS crates.io API. Every publication first validates the artifact and performs a fresh version lookup. An existing exact checksum returns `Reused` without authentication; a conflicting checksum fails. Only a structured JSON 404 means absence. Authentication failures, transport failures, malformed metadata, incorrect identities, and invalid checksums are errors, not permission to upload.

For a missing version, publication sends one PUT to `/api/v1/crates/new` with the validated metadata and the retained bytes, each prefixed by its checked little-endian `u32` length. It uses Cargo's raw token authorization header, validates the structured success and warning fields, and disables HTTP redirects and automatic transport retries. A disconnected or malformed upload response has an ambiguous outcome: reconcile the registry before considering another upload. Acceptance is separate from index visibility; this provider does not turn a visibility delay into another upload.

Downloads use unauthenticated, nonredirecting requests to the known public static crate location and compare the received bytes with fresh registry checksum metadata. Registry credentials are never forwarded to an artifact host.

## Authentication

Authentication is owned by `publish`, not by workflow YAML or artifact receipts. The production provider first uses a nonempty `CARGO_REGISTRY_TOKEN`; otherwise a GitHub execution provider can exchange its scoped Actions identity for a short-lived crates.io token. The identity request requires `ACTIONS_ID_TOKEN_REQUEST_URL`, `ACTIONS_ID_TOKEN_REQUEST_TOKEN`, and `id-token: write`, and requests audience `crates.io`. The exchange follows the official crates.io trusted publishing token API.

Tokens are masked for Actions, omitted from diagnostics and serialization, and never stored in execution metadata. Authentication responses and identity challenge URLs are not displayed. Short-lived tokens are revoked after publication attempts, including ordinary upload failures. A cleanup failure preserves the original upload error; after accepted publication it produces a safe warning without changing `Published` into a retryable failure.

Local execution without an explicit production token fails clearly. Trusted publishing requires separately authorized crates.io configuration and an already published crate; implementing this provider does not configure credentials, perform an exchange, or publish anything.

## Isolated Registry Tests

`CratesIoPublisher::with_endpoint(endpoint: &str, token: Option<String>) -> Result<CratesIoPublisher>` accepts only a bare literal loopback IP endpoint. Its optional token is an explicitly supplied simulator credential; this constructor never reads production token variables or requests a live Actions identity. Redirects and proxy use are disabled.

Inline tests create real library and binary Cargo fixtures, preserve manifest and lockfile bytes across version changes, verify extracted consumers, and reject conflicting or corrupt artifacts. Local TCP simulators capture the complete upload frame and compare metadata, archive bytes, and SHA-256 with the retained artifact. They also exercise existing-version reconciliation, structured absence, authentication errors, malformed responses, disconnected uploads without retries, public downloads, and simulated short-lived token cleanup. No test publishes to a real registry or requests live credentials.
