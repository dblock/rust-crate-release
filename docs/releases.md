# Release Workflows

The library, [CLI](cli.md), and [GitHub Actions integration](actions.md) use the same typed numeric/semantic release engine. Follow the [trusted-publishing setup](actions.md) for a standard Cargo project; use the Rust API below when you need custom providers, typed hooks, or workflow composition.

For file-editing callbacks, start with [Custom Hooks](#custom-hooks). Provider reference: [Cargo and Registries](cargo.md), [Git Repositories](providers.md), and [GitHub Hosting](hosting.md). [Typed Workflows](workflows.md) covers composing your own steps.

## Configure Providers

Construct `engine::ReleaseWorkflow::new(repository, package, publisher, store)` with explicit `Arc` providers. Defaults are numeric releases, automatic changelog detection, UTC dates, real Cargo format/test/Clippy validation, next patch development, local execution/preservation, bounded registry verification, and no hosted release. `AutomaticChangelog` detects Keep a Changelog or Intridea and skips absent files. Assign `KeepAChangelog`, `IntrideaChangelog`, or `NoChangelog` to `components.changelog` for an explicit choice.

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

## Custom Hooks

Use hooks when you need project-specific work, such as updating a `VERSION` file. For file edits, use `prepare` before publication or `next` after publication; the engine includes those edits in the appropriate validation and commit. Start with the [runnable example](#edit-a-file-in-a-hook) below.

| Hook | Runs | Source Edit Result |
| --- | --- | --- |
| `prepare.before` | Before release manifest/changelog edits. | Included in source validation and the release commit/archive. |
| `prepare.after` | After release manifest/changelog edits, before staging. | Included in source validation and the release commit/archive. |
| `next.before` | After publication/push/optional hosting, before development edits. | Included in development validation and the next-development commit. |
| `next.after` | After development manifest/changelog edits, before staging. | Included in development validation and the next-development commit. |

Implement `LifecycleHook<Analyzed, Edited>` for preparation, or `LifecycleHook<Hosted, NextEdited>` for next development, and assign `workflow.hooks.prepare = workflow.hooks.prepare.with(hook)` or the corresponding `next` registry. The type parameters refer to the inner edit operation, not the later staging/commit steps. Read the updated manifest in an after hook when generating version-dependent files, rather than assuming that the released version is still the checkout's current development version.

Next hooks also run when next-development preparation is disabled; check `NextEdited::next_version()` before performing an operation that requires a next version. Hook failures propagate, and recovery may invoke hooks again, so keep edits repeatable and do not move HEAD, stage, or commit inside a hook. Source edits in publication/verification/other phases are not supported.

### Edit a File in a Hook

This example keeps `VERSION` synchronized with `Cargo.toml` before and after publication. It is a standalone local Rust runner. To use the hooks with the standard CLI and Actions workflow instead, register them in a [project runner](cli.md#project-runners) using `cli::main_with`.

Add the library as a development dependency:

```sh
cargo add --dev rust-crate-release@0.1.1
```

Create `examples/release.rs`:

```no_run
use rust_crate_release::{
    Result,
    engine::{Analyzed, Edited, Hosted, NextEdited, ReleaseRequest, ReleaseWorkflow},
    execution::ExecutionContext,
    git::{Git, ReleaseRepository},
    hooks::LifecycleHook,
    package::{CargoPackage, PackageProject},
    registry::CratesIoPublisher,
    store::DirectoryReleaseStore,
};
use std::{fs, sync::Arc};

struct VersionFile;

impl VersionFile {
    fn update(&self) -> Result {
        let package = CargoPackage::new(".")?;
        fs::write(
            package.root().join("VERSION"),
            format!("{}\n", package.metadata()?.version),
        )?;
        Ok(())
    }
}

impl LifecycleHook<Analyzed, Edited> for VersionFile {
    fn id(&self) -> &str {
        "release-version-file"
    }

    fn after(&self, _: &Edited, _: &ExecutionContext) -> Result {
        self.update()
    }
}

impl LifecycleHook<Hosted, NextEdited> for VersionFile {
    fn id(&self) -> &str {
        "development-version-file"
    }

    fn after(&self, _: &NextEdited, _: &ExecutionContext) -> Result {
        self.update()
    }
}

fn main() -> Result {
    let repository = Arc::new(Git::new(".", "main", "origin")?);
    let store = Arc::new(DirectoryReleaseStore::new(
        repository.artifact_directory()?,
    )?);
    let mut workflow = ReleaseWorkflow::new(
        repository,
        Arc::new(CargoPackage::new(".")?),
        Arc::new(CratesIoPublisher::new()?),
        store,
    );
    workflow.hooks.prepare = workflow.hooks.prepare.with(VersionFile);
    workflow.hooks.next = workflow.hooks.next.with(VersionFile);

    workflow.release(ReleaseRequest::default())?;
    Ok(())
}
```

The `prepare.after` hook runs after the engine updates the release manifest but **before validation, the release commit, packaging, and publication**. The `next.after` hook runs after publication and the release push, once the engine has updated the development manifest; its edits are validated, committed, and pushed in the next-development commit. Both read the current manifest so repeated calls write the same contents. Implement `before` instead of `after` to run before that stage's built-in operation.

Commit the example and dependency/lockfile updates. Follow the [local-publication prerequisites](cli.md#local-publication), including `CARGO_REGISTRY_TOKEN` and Git push authentication. From the project root on your clean default branch, run:

```sh
cargo run --example release
```

**This publishes and pushes a real release.** Replace `main` in the example if your default branch differs. The runner uses the library defaults: automatic changelog detection, next patch development, and no GitHub Release hosting. It does not read `rcr.toml`. This is a local runner, not a replacement for the [trusted-publishing workflow](actions.md).

The `VERSION` file contains the released version in the release commit/archive and the next version in the development commit. If your manifest has a package `include` allowlist, include `VERSION`. To exercise only preparation without publication or pushing, replace `workflow.release(...)` with `workflow.prepare(...)`; this still edits, validates, commits, and tags locally. Preserve the resulting captured inputs explicitly before a later publication, as shown in [Configure Providers](#configure-providers).

Only preparation and next-development hooks may edit source files. Do not edit source in `publish.after`, `verify.after`, or other late hooks: those cannot change an already-built/published archive and can fail clean-tree checks or leave uncommitted edits. Hook errors stop the workflow; hooks may run again during recovery, so avoid non-idempotent operations such as blindly appending text.

### Observe Authentication and Git Operations

Fine-grained `tag: LifecycleHooks<Committed, Committed>` hooks surround local tag creation after archive verification. `authenticate: LifecycleHooks<Preserved, Preserved>` hooks surround the registry credential exchange before upload; they never receive credentials and crates.io skips them when reusing an existing version. An authentication after-hook failure revokes any issued short-lived token without uploading. Custom publishers can override `publish_with_authentication` to identify their own authentication boundary; the default surrounds their no-op authentication.

`commit_next: LifecycleHooks<NextEdited, Development>` and `push_next: LifecycleHooks<Development, Development>` surround the validated development commit and push separately. They do not run when next development is disabled. These observation hooks must not edit source or move refs. Hook failures stop subsequent mutations, including a failed development commit after-hook stopping the development push.

## Test a Consumer Release

`testing::ReleaseFixture` runs the production CLI/configuration/engine against a real disposable Git remote, `LocalRegistry`, and `DirectoryReleaseHost`. Create its project with `GitProject::from_project(source, &CopySelection::Paths(files))?` to copy reviewed manifests, lockfile, source, `rcr.toml`, and runner files without copying Git metadata or build output. Copy selection is explicit; existing project `.gitignore` contents are preserved.

```no_run
use std::path::{Path, PathBuf};
use rust_crate_release::{Result, testing::{CopySelection, GitProject, ReleaseFixture}};

fn check_release() -> Result {
    let files = ["Cargo.toml", "Cargo.lock", "src/lib.rs", "rcr.toml", "CHANGELOG.md"]
        .map(PathBuf::from).to_vec();
    let project = GitProject::from_project(Path::new("."), &CopySelection::Paths(files))?;
    let mut fixture = ReleaseFixture::new(project)?;
    fixture.configure_workflow(|workflow| {
        // Register the same project-owned hooks used by the production runner.
        let _ = workflow;
        Ok(())
    });
    let released = fixture.release(None)?;
    assert_eq!(released["verification"]["status"], "verified");
    let release_manifest = fixture.remote_file("v0.1.0", Path::new("Cargo.toml"))?;
    let development_manifest = fixture.remote_file("master", Path::new("Cargo.toml"))?;
    assert_ne!(release_manifest, development_manifest);
    Ok(())
}
```

`configure_workflow` receives the actual configured production builder. After configuration, the facade binds publication, hosting, execution, preservation, and bounded verification to its local adapters. `run(&args)` accepts ordinary CLI arguments, including explicit preparation/input transport; no receipt authorizes recovery. `remote_file(reference, path)` returns exact bytes from a committed remote ref, not the current working tree, and rejects traversal/Git metadata paths.

For the production runner in a separate process, call `fixture.process(program, prefix_args, &["release"])`. For example, use `cargo` plus `["run", "--locked", "--example", "release", "--"]`. Explicit test registry/host flags isolate both destinations, including a configured production GitHub repository; configured production publish commands are suppressed and publication credentials/OIDC inputs are removed. A trusted runner must retain these selected adapters. The process receives `RCR_FIXTURE_ACTIVE=1`; begin a consumer release integration test with `if testing::in_release_fixture() { return Ok(()); }` so child Cargo validation does not recursively release. In-process fixtures should exclude their own release integration test from validation, for example using `cargo test --locked --lib`. Other project validation still runs normally. Fixtures are not a sandbox for malicious trusted callbacks/providers.

## Capture Inputs Once

A fresh request captures current package metadata, source commit, initial remote head, source timestamp, and release date. Dirty checkouts, wrong branches, source/remote divergence, conflicting tags, invalid versions, and nonreleasable semantic decisions fail closed. Dates use the configured IANA timezone; archives receive the captured source timestamp. Numeric overrides may increase the source manifest version but never decrease it.

Separate preparation/publication calls pass `prepared.captured().clone()` explicitly in `ReleaseRequest::captured`. This is immutable source/analysis data, not a workflow descriptor: it contains no phase, completed-step list, or skip authorization. The engine validates source ancestry, timestamp, package identity, version override, semantic decision, and actual outputs again. It never loads these inputs from a diagnostic receipt. Custom callers must keep captured data within their intended invocation rather than automatically restoring it for a fresh dispatch.

Semantic preparation retains the original source-bound decision; publication does not reanalyze against its newly created local tag. With no eligible commits and no valid override, analysis fails before source edits. Reusing the workflow object does not preserve prior invocation inputs or execution history.

## Validate and Preserve Exact Bytes

`CommandValidator` runs trusted argv arrays directly, without shell interpolation. Its defaults execute `cargo fmt --all -- --check`, `cargo test --locked`, and `cargo clippy --locked --all-targets -- -D warnings`. Output goes to stderr; command failures preserve both output channels. Configure another validator for project-specific lint or integration commands.

`ReleaseRequest::archive` selects a nonempty explicit archive. Absent and empty paths both select normal packaging or matching retained output. Invalid overrides fail instead of rebuilding silently. Retained/overridden archives must match captured name/version and the prepared clean VCS commit, and the actual extracted-archive consumer runs on every preparation. Matching output is reused without repacking; conflicting bytes fail.

Build retains `<name>-<version>.crate`, `<name>-<version>.bundle`, `release.json`, and credential-free `execution.json`. `ArtifactPreserver::preserve` must succeed before the registry publisher is called. `LocalArtifacts` refuses GitHub execution: Actions callers must supply independently verified durable preservation, not a completion flag. The standard composite uploads recovery files between preparation and release, then uses `actions::GitHubArtifacts` to compare the stored snapshot before publication. The engine cannot prove arbitrary custom providers honest.

## Reconcile Partial Releases

Registry acceptance is followed by atomic branch/tag push, hosted metadata/assets, validated next-development edits, and final verification. Verification binds the released version and retained bytes even after the manifest advances. Only structured missing-version responses receive bounded fresh lookups. A visibility timeout yields `Verification::NotVisible` without blocking hosting/development or repeating upload; transport, authentication, identity, integrity, and malformed responses remain failures.

Repeat with the same explicit captured inputs and matching outputs to complete missing operations without republishing, moving tags, or rewriting a remote descendant. Receipts annotate progress/errors but never authorize skipping. Fresh requests still require a clean checkout matching its remote.

Failed source/next validation deliberately leaves edits intact. Repair and validate them explicitly, retain the intended clean commit, and then repeat with the original captured inputs; do not reset away unexpected user/validator edits. A dirty checkout is not silently accepted as a resumable state.

Standalone readonly verification uses `ReleaseVerifier::verify(release, archive, publisher)` and needs neither current manifest metadata nor current HEAD. It does not publish, push, host, advance development, or mutate the supplied receipt. See [Diagnostics and Recovery](recovery.md) for retained-evidence validation.
