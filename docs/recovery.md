# Diagnostics and Recovery

## Retain Evidence

`ReleaseStore` separates filesystem diagnostics from registry, Git, and hosting providers. `DirectoryReleaseStore` atomically replaces `release.json` inside an explicitly selected directory, using a private temporary file and private file permissions on Unix. Other retained files are preserved. Git's default artifact directory is private repository metadata, not a tracked source directory.

`ReleaseReceipt` records immutable release identity, source timestamp, initial remote head, prepared tree, release/development commits, retained archive identity, current phase, verification, and an optional error. Initialization failures can retain diagnostics without a release identity. Validation rejects malformed object IDs, conflicting tags, invalid versions, archive traversal, and checksums detached from their artifact.

These are diagnostic observations, not a recovery protocol: `phase = complete` does not prove that publication or any other operation happened. Never use a receipt to skip execution, authorize a registry mutation, restore a workflow descriptor, reset a checkout, or choose an older release automatically. A fresh release must use its current inputs and inspect actual outputs independently.

## Verify the Explicit Archive

`DirectoryReleaseStore::retained_artifact(receipt)` reads only the captured canonical archive filename in that store. It reopens and validates the real `.crate`, rejects symbolic links, and compares package identity, SHA-256, and size to the receipt. It never uses the working manifest's now-current version. Damaged or missing evidence fails rather than being silently rebuilt or accepted from a claimed checksum.

Registry acceptance, remote Git refs, hosted-release metadata, and next-development changes are independent outputs. Matching registry checksums and immutable tags are reusable; conflicts fail closed. A remote descendant containing a release is not a reason to rewind it. Git/hosting providers reconcile only their own observable outputs, and local test adapters never silently replace the production publisher.

## Handle Partial Success

Inspect the retained archive/bundle, registry version metadata, remote release tag, hosted asset, and current checkout before retrying. An upload timeout may follow acceptance; never assume it means the version was not published. Delayed registry visibility must use bounded fresh lookups, not upload retries. Authentication, transport, malformed metadata, identity, and integrity errors are failures rather than absence.

No automatic publication/recovery command exists yet. The integrated engine and future `rcr verify` will use these provider boundaries; until then, diagnostics are library APIs and the release process remains separately gated in [Releasing](../RELEASING.md).
