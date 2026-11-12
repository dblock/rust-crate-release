<img src="assets/rust-crate-release.svg" alt="An open Cargo crate with an upward release arrow" width="160" height="160"> <!-- rumdl-disable-line MD033 MD041 -->
<!-- Logo-first layout and explicit image sizing are intentional. -->

# rust-crate-release

[![Test](https://github.com/dblock/rust-crate-release/actions/workflows/test.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/test.yml) [![Lint](https://github.com/dblock/rust-crate-release/actions/workflows/lint.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/lint.yml) [![Package](https://github.com/dblock/rust-crate-release/actions/workflows/package.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/package.yml)

Release Rust crates to crates.io with trusted publishing, checked archives, changelogs, Git tags, and GitHub Releases. Use the built-in rcr CLI and GitHub Actions workflow, or compose typed release workflows in Rust. Ported from [ruby-gem-release](https://github.com/dblock/ruby-gem-release) and [npm-package-release](https://github.com/dblock/npm-package-release).

## Installation

For local commands, install rcr with Rust 1.99.1 or later:

```sh
cargo install --locked rust-crate-release --version 0.1.0
```

The GitHub Actions workflow installs the tool itself; no local installation is required to use it.

## Usage

Add `.github/workflows/release.yml` to your Cargo project:

```yaml
name: Release

on:
  workflow_dispatch:

permissions:
  actions: read
  contents: write
  id-token: write

jobs:
  release:
    uses: dblock/rust-crate-release/.github/workflows/release.yml@v0.1.0
    with:
      tool-ref: v0.1.0
```

Complete the [trusted-publishing setup](docs/actions.md): publish your crate's first version if needed, configure its crates.io publisher for your repository's `release.yml` and `release` environment, and allow the workflow's release commits and tags. Then run the Release workflow on your default branch. Do not store a Cargo token secret or add a separate authentication action; rcr exchanges OIDC credentials itself.

Each release validates the project and the actual `.crate`, durably preserves recovery files, publishes those exact bytes, pushes the release commit/tag, optionally creates a GitHub Release, and prepares the next development version. A failed preservation check prevents publication.

To prepare locally without publishing or pushing:

```sh
rcr prepare --save-inputs .git/rcr/source-inputs.json
```

Preparation still edits, validates, commits, and tags locally; it is not a read-only dry run. See [CLI and Configuration](docs/cli.md) for local publication and verification commands.

## Configuration

For a project hosted at `acme/widget` with default branch `main`, add `rcr.toml`:

```toml
default-branch = "main"
github-repository = "acme/widget"
```

Replace both values with your project's details. Numeric releases use the manifest version, `CHANGELOG.md` with a meaningful `[Unreleased]` section, and a next patch version by default. Keep `Cargo.lock` committed. [CLI and Configuration](docs/cli.md) covers changelog formats, semantic releases, validation commands, and version policies. Use the [Rust library](docs/releases.md) from your own runner for custom typed components, hooks, and observers.

## Custom Hooks

For example, keep a `VERSION` file synchronized with `Cargo.toml`: write the release version before publishing, then the next development version after publishing. Hooks are Rust code registered in your own runner. The example below constructs the engine directly; [project runners](docs/cli.md#project-runners) using `cli::main_with` let `rcr.toml`, the installed rcr command, and the supplied Actions workflow select the same custom hooks.

Add the library as a development dependency:

```sh
cargo add --dev rust-crate-release@0.1.0
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

Commit the example, dependency/lockfile updates, and meaningful changelog notes. Follow the [local-publication prerequisites](docs/cli.md#local-publication), including `CARGO_REGISTRY_TOKEN` and Git push authentication. From the project root on your clean default branch, run:

```sh
cargo run --example release
```

**This publishes and pushes a real release.** Replace `main` in the example if your default branch differs. The runner uses the library defaults: Keep a Changelog, next patch development, and no GitHub Release hosting. It does not read `rcr.toml`. This is a local runner, not a replacement for the trusted-publishing workflow above.

The `VERSION` file contains the released version in the release commit/archive and the next version in the development commit. If your manifest has a package `include` allowlist, include `VERSION`. To exercise only preparation without publication or pushing, replace `workflow.release(...)` with `workflow.prepare(...)`; this still edits, validates, commits, and tags locally. Preserve the resulting captured inputs explicitly before a later publication, as shown in [Release Workflows](docs/releases.md#configure-providers).

Only preparation and next-development hooks may edit source files. Do not edit source in `publish.after`, `verify.after`, or other late hooks: those cannot change an already-built/published archive and can fail clean-tree checks or leave uncommitted edits. Hook errors stop the workflow; hooks may run again during recovery, so avoid non-idempotent operations such as blindly appending text. See [Release Workflows](docs/releases.md) for all hook stages and provider options.

## Contributing

Read [Contributing](CONTRIBUTING.md), [Design](DESIGN.md), [Changelog](CHANGELOG.md), [Releasing](RELEASING.md), [Agent Instructions](AGENTS.md), [CLI and Configuration](docs/cli.md), [GitHub Actions](docs/actions.md), [Typed Workflows](docs/workflows.md), [Changelogs](docs/changelogs.md), [Semantic Releases](docs/semantic.md), [Cargo Providers](docs/cargo.md), [Release Providers](docs/providers.md), [GitHub Hosting](docs/hosting.md), [Release Workflows](docs/releases.md), and [Diagnostics and Recovery](docs/recovery.md).

## License

[MIT](https://github.com/dblock/rust-crate-release/blob/master/LICENSE).
