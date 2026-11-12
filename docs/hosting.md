# GitHub Release Hosting

## Provider API

`hosting::ReleaseHost: Send + Sync` exposes `publish(&self, release: &Release, commit: &str, notes: &str, artifact: &Artifact) -> Result`. `GitHubReleaseHost::new("owner/repository")` selects the public GitHub REST API without reading credentials or making requests. Import the trait to call `publish`.

```rust
use rust_crate_release::hosting::{GitHubReleaseHost, ReleaseHost};
use rust_crate_release::package::Artifact;
use rust_crate_release::{Release, Result};

fn host(release: &Release, commit: &str, notes: &str, artifact: &Artifact) -> Result {
    GitHubReleaseHost::new("owner/repository")?.publish(release, commit, notes, artifact)
}
```

The engine must already have pushed and verified the tag. Hosting never creates, moves, or deletes Git refs. The explicit `commit` must be a complete Git object ID; it can differ from the captured pre-preparation source commit. The provider verifies archive identity against the immutable released name/version, rechecks the local archive, and retains the exact checked bytes for upload.

## Authentication and Endpoints

Publication reads `GH_TOKEN`, then `GITHUB_TOKEN`, then privately captures `gh auth token --hostname github.com`. Credentials are neither persisted nor included in provider diagnostics. The caller supplies GitHub repository contents permission; this provider does not acquire or revoke registry credentials.

Authenticated API calls use only `https://api.github.com` and the derived `https://uploads.github.com/repos/{owner}/{repository}/releases/{id}/assets` endpoint. Repository names are validated as literal owner/name segments. Returned upload URLs are ignored. Automatic redirects and retries are disabled, requests have bounded connection/operation timeouts, and environment HTTP proxies are not used.

When GitHub omits an asset digest, the provider downloads through the authenticated asset API. Signed download redirects use fresh unauthenticated GET requests, require HTTPS in production, reject embedded credentials, and are bounded. Downloaded bytes cannot exceed the retained archive length. Raw error bodies, tokens, and signed URLs are not included in errors.

## Reconciliation and Failure Recovery

Only a structured GitHub `404` with `"message": "Not Found"` authorizes creating a missing release. Authentication, transport, malformed metadata, and other errors fail closed. Creation uses the captured tag, explicit commit, tag as title, supplied notes, and `draft: false`/`prerelease: false`; existing releases are patched only for differing expected metadata. Changing release metadata does not move an existing tag.

The canonical asset is `{name}-{version}.crate`. A matching SHA-256 digest and size reuses it; if no digest is available, the downloaded bytes must match. Duplicate canonical assets fail closed. Only a differing canonical asset is deleted and replaced, and unrelated assets are preserved. Fresh release metadata and the canonical asset are verified after reconciliation.

Mutations are never automatically retried. Failures can leave a created release, updated notes, a removed old asset, or an uploaded asset whose response was lost; invoke publication again to reconcile actual hosted state. Hosting does not roll back Git refs, registry publication, or unrelated assets. Asset listings are paginated with a bounded 100-page limit, and eventual visibility failures require a subsequent reconciliation rather than an implicit mutation retry.

## Isolated Tests

`GitHubReleaseHost::for_test_server(repository, endpoint)` accepts only a bare HTTP(S) origin with a literal loopback IP address and always uses a fixed dummy token. It never reads environment credentials or invokes `gh`; download redirects must remain on that exact test origin. Inline behavior tests use a bounded local TCP simulator and real Cargo archives from disposable Git projects, never live GitHub or registry publication.
