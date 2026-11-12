# Release Workflows

The library composes numeric and semantic releases through the same typed engine. The CLI and Actions wrappers follow separately; the examples below are Rust APIs, not authorization to publish.

## Configure Providers

Construct `engine::ReleaseWorkflow::new(repository, package, publisher, store)` with explicit `Arc` providers. Defaults are numeric releases, Keep a Changelog in `CHANGELOG.md`, UTC dates, real Cargo format/test/Clippy validation, next patch development, local execution/preservation, bounded registry verification, and no hosted release.

Its `components` expose replaceable package, registry, repository, changelog, clock, validator, next-version policy, hosting, execution, artifact-preservation, verification, and diagnostic providers. `semantic(history)` selects Conventional Commits analysis and generated notes, with next-development changes disabled. Override components before constructing or invoking a workflow; existing compositions retain their captured component choices.

```no_run
use std::sync::Arc;
use rust_crate_release::{
    Result,
    changelog::IntrideaChangelog,
    engine::{ReleaseRequest, ReleaseWorkflow},
    git::{Git, ReleaseRepository},
    package::CargoPackage,
    registry::CratesIoPublisher,
    store::DirectoryReleaseStore,
};

fn release_project() -> Result {
    let repository = Arc::new(Git::new(".", "master", "origin")?);
    let store = Arc::new(DirectoryReleaseStore::new(repository.artifact_directory()?)?);
    let mut workflow = ReleaseWorkflow::new(
        repository,
        Arc::new(CargoPackage::new(".")?),
        Arc::new(CratesIoPublisher::new()?),
        store,
    );
    workflow.components.changelog = Arc::new(IntrideaChangelog::default());
    let prepared = workflow.prepare(ReleaseRequest::default())?;
    let request = ReleaseRequest {
        captured: Some(prepared.captured().clone()),
        ..ReleaseRequest::default()
    };
    let released = workflow.release(request)?;
    assert_eq!(released.release().version(), prepared.release().version());
    Ok(())
}
```

Real publication requires separately authorized credentials and remote-write permissions. Tests inject `testing::LocalRegistry` and `testing::DirectoryReleaseHost` instead.

## Typed Lifecycle

`prepare_workflow()` composes capture, analyze, prepare, validate, commit, and build. Its `Packaged` result includes the immutable release, captured inputs, notes, release commit, and verified archive; a local immutable tag and Git bundle are retained. It does not publish, push, host, prepare development, or report whole-release success.

`release_workflow()` repeats those operations and adds durable preservation, publication, atomic release push, optional hosting, optional next development, and explicit-version verification. Each public step implements `Step<Input>` with a distinct output marker. Markers cannot be fabricated through public fields; arbitrary custom providers and callbacks still remain trusted. `prepare` and `release` run these compositions with diagnostics and configured observers.

`ReleaseHooks` has a typed ordered registry for every step. Before/after hooks receive borrowed input/output and immutable execution snapshots. Both before and after preparation/next hooks may make deliberate source edits: their `Edited`/`NextEdited` output precedes trusted staging, real validation, and commitment. Other phases must not edit source. Validators must not change tracked, staged, or untracked release files; unexpected edits are preserved and rejected before commitment. A disabled host or next-development operation records `Skipped`, including through a hook wrapper.

## Capture Inputs Once

A fresh request captures current package metadata, source commit, initial remote head, source timestamp, and release date. Dirty checkouts, wrong branches, source/remote divergence, conflicting tags, invalid versions, and nonreleasable semantic decisions fail closed. Dates use the configured IANA timezone; archives receive the captured source timestamp. Numeric overrides may increase the source manifest version but never decrease it.

Separate preparation/publication calls pass `prepared.captured().clone()` explicitly in `ReleaseRequest::captured`. This is immutable source/analysis data, not a workflow descriptor: it contains no phase, completed-step list, or skip authorization. The engine validates source ancestry, timestamp, package identity, version override, semantic decision, and actual outputs again. It never loads these inputs from a diagnostic receipt. Custom callers must keep captured data within their intended invocation rather than automatically restoring it for a fresh dispatch.

Semantic preparation retains the original source-bound decision; publication does not reanalyze against its newly created local tag. With no eligible commits and no valid override, analysis fails before source edits. Reusing the workflow object does not preserve prior invocation inputs or execution history.

## Validate and Preserve Exact Bytes

`CommandValidator` runs trusted argv arrays directly, without shell interpolation. Its defaults execute `cargo fmt --all -- --check`, `cargo test --locked`, and `cargo clippy --locked --all-targets -- -D warnings`. Output goes to stderr; command failures preserve both output channels. Configure another validator for project-specific lint or integration commands.

`ReleaseRequest::archive` selects a nonempty explicit archive. Absent and empty paths both select normal packaging or matching retained output. Invalid overrides fail instead of rebuilding silently. Retained/overridden archives must match captured name/version and the prepared clean VCS commit, and the actual extracted-archive consumer runs on every preparation. Matching output is reused without repacking; conflicting bytes fail.

Build retains `<name>-<version>.crate`, `<name>-<version>.bundle`, `release.json`, and credential-free `execution.json`. `ArtifactPreserver::preserve` must succeed before the registry publisher is called. `LocalArtifacts` refuses GitHub execution: Actions callers must inject a real durable uploader, not a completion flag. The engine cannot prove arbitrary custom providers honest.

## Reconcile Partial Releases

Registry acceptance is followed by atomic branch/tag push, hosted metadata/assets, validated next-development edits, and final verification. Verification binds the released version and retained bytes even after the manifest advances. Only structured missing-version responses receive bounded fresh lookups. A visibility timeout yields `Verification::NotVisible` without blocking hosting/development or repeating upload; transport, authentication, identity, integrity, and malformed responses remain failures.

Repeat with the same explicit captured inputs and matching outputs to complete missing operations without republishing, moving tags, or rewriting a remote descendant. Receipts annotate progress/errors but never authorize skipping. Fresh requests still require a clean checkout matching its remote.

Failed source/next validation deliberately leaves edits intact. Repair and validate them explicitly, retain the intended clean commit, and then repeat with the original captured inputs; do not reset away unexpected user/validator edits. A dirty checkout is not silently accepted as a resumable state.

Standalone readonly verification uses `ReleaseVerifier::verify(release, archive, publisher)` and needs neither current manifest metadata nor current HEAD. It does not publish, push, host, advance development, or mutate the supplied receipt. See [Diagnostics and Recovery](recovery.md) for retained-evidence validation.
