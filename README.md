# rust-crate-release

[![Test](https://github.com/dblock/rust-crate-release/actions/workflows/test.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/test.yml) [![Lint](https://github.com/dblock/rust-crate-release/actions/workflows/lint.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/lint.yml) [![Package](https://github.com/dblock/rust-crate-release/actions/workflows/package.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/package.yml)

Composable release workflows for Rust crates, ported from [ruby-gem-release](https://github.com/dblock/ruby-gem-release) and [npm-package-release](https://github.com/dblock/npm-package-release).

## Installation

This is an unpublished Rust library requiring Rust 1.99.1 or later. To work on it:

```sh
git clone https://github.com/dblock/rust-crate-release.git
cd rust-crate-release
cargo test --locked --workspace
```

## Usage

Start with [Release Workflows](docs/releases.md) for the typed numeric/semantic release engine, or [Typed Workflows](docs/workflows.md) to compose custom steps and hooks. The `rcr` executable, `rcr.toml`, and publishing Actions are not implemented yet. See [Design](DESIGN.md) for the full port scope.

## Configuration

Configure `ReleaseWorkflow::components`, `hooks`, and `observers` in a project-owned Rust runner. [Release Workflows](docs/releases.md) documents provider choices, defaults, validation, and explicit captured inputs.

## Contributing

Read [Contributing](CONTRIBUTING.md), [Design](DESIGN.md), [Changelog](CHANGELOG.md), [Releasing](RELEASING.md), [Agent Instructions](AGENTS.md), [Typed Workflows](docs/workflows.md), [Changelogs](docs/changelogs.md), [Semantic Releases](docs/semantic.md), [Cargo Providers](docs/cargo.md), [Release Providers](docs/providers.md), [GitHub Hosting](docs/hosting.md), [Release Workflows](docs/releases.md), and [Diagnostics and Recovery](docs/recovery.md).

## License

[MIT](https://github.com/dblock/rust-crate-release/blob/master/LICENSE).
