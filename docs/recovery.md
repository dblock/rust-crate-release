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

`rcr release --inputs PATH` reconciles actual outputs using the deliberately supplied original source inputs; it is not an automatic receipt-restoration command. The source, archive, registry, and refs must still satisfy the engine's guards. Failed source or next-development validation leaves edits intact for inspection and deliberate repair; never reset them away just to make a retry pass.

GitHub source inputs are bound to repository/workflow/run/attempt. A new dispatch or attempt captures its own inputs and cannot automatically resume an earlier attempt's downloaded file. Do not blindly start another release after crates.io accepted a version but a push or hosting step failed. Inspect which version was accepted and which outputs are missing before choosing a controlled same-attempt retry or deliberate operator recovery.

## Verify Without Publishing

`rcr verify` compares an explicit released name/version with a retained archive, independent of the current manifest version or HEAD:

```sh
rcr verify --name widget --release-version 0.1.1 \
  --archive ../widget-release-artifacts/widget-0.1.1.crate
```

The command is read-only: it does not publish, push, create hosted assets, edit the checkout, or update receipts. A bounded `not_visible` result means the released version could not yet be confirmed; run verification again later rather than publishing again. Authentication, network, malformed metadata, identity, and checksum errors remain failures.

In Actions, download the immutable `rcr-release-<run-id>-<attempt>` snapshot and final `rcr-diagnostics-<run-id>-<attempt>` outside your checkout. The former contains the verified pre-publication crate, Git recovery bundle, and source inputs; final diagnostics may record a later phase or error. Neither archive's receipt authorizes skipping work. [GitHub Actions and Trusted Publishing](actions.md#run-and-verify-a-release) shows download commands and setup checks.
