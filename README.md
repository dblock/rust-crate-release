<img src="assets/rust-crate-release.svg" alt="An open Cargo crate with an upward release arrow" width="160" height="160"> <!-- rumdl-disable-line MD033 MD041 -->
<!-- Logo-first layout and explicit image sizing are intentional. -->

# rust-crate-release

[![Test](https://github.com/dblock/rust-crate-release/actions/workflows/test.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/test.yml) [![Lint](https://github.com/dblock/rust-crate-release/actions/workflows/lint.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/lint.yml) [![Package](https://github.com/dblock/rust-crate-release/actions/workflows/package.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/package.yml)

Composable release workflows for Rust crates, ported from [ruby-gem-release](https://github.com/dblock/ruby-gem-release) and [npm-package-release](https://github.com/dblock/npm-package-release).

## Installation

This is an unpublished Rust library and CLI requiring Rust 1.99.1 or later. Install from a reviewed checkout:

```sh
git clone https://github.com/dblock/rust-crate-release.git
cd rust-crate-release
cargo install --locked --path .
```

## Usage

With a clean default-branch checkout and meaningful changelog:

```sh
rcr prepare --save-inputs .git/rcr/source-inputs.json
```

Preparation validates, commits, packages, and tags locally; it does not publish or push. See [CLI and Configuration](docs/cli.md) for commands, [GitHub Actions](docs/actions.md) for publishing integration, [Release Workflows](docs/releases.md) for the typed engine, and [Typed Workflows](docs/workflows.md) for custom steps/hooks.

## Configuration

Use trusted data-only `rcr.toml` for built-in numeric/semantic behavior; [CLI and Configuration](docs/cli.md) lists defaults and options. Configure typed components/hooks/observers in a project-owned Rust runner for custom workflows.

## Contributing

Read [Contributing](CONTRIBUTING.md), [Design](DESIGN.md), [Changelog](CHANGELOG.md), [Releasing](RELEASING.md), [Agent Instructions](AGENTS.md), [CLI and Configuration](docs/cli.md), [GitHub Actions](docs/actions.md), [Typed Workflows](docs/workflows.md), [Changelogs](docs/changelogs.md), [Semantic Releases](docs/semantic.md), [Cargo Providers](docs/cargo.md), [Release Providers](docs/providers.md), [GitHub Hosting](docs/hosting.md), [Release Workflows](docs/releases.md), and [Diagnostics and Recovery](docs/recovery.md).

## License

[MIT](https://github.com/dblock/rust-crate-release/blob/master/LICENSE).
