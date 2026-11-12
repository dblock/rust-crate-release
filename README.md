# rust-crate-release

[![Test](https://github.com/dblock/rust-crate-release/actions/workflows/test.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/test.yml) [![Lint](https://github.com/dblock/rust-crate-release/actions/workflows/lint.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/lint.yml) [![Package](https://github.com/dblock/rust-crate-release/actions/workflows/package.yml/badge.svg)](https://github.com/dblock/rust-crate-release/actions/workflows/package.yml)

Composable release workflows for Rust crates, ported from [ruby-gem-release](https://github.com/dblock/ruby-gem-release) and [npm-package-release](https://github.com/dblock/npm-package-release).

## Installation

This is an unpublished scaffold requiring Rust 1.99.1 or later. To work on it:

```sh
git clone https://github.com/dblock/rust-crate-release.git
cd rust-crate-release
cargo test --locked --workspace
```

## Usage

Release functionality is not implemented yet. The agreed first implementation includes a Rust library, the `rcr` executable, `rcr.toml` configuration, and GitHub Actions interfaces with full Ruby/npm feature parity; see [Design](DESIGN.md).

## Contributing

Read [Contributing](CONTRIBUTING.md), [Design](DESIGN.md), [Changelog](CHANGELOG.md), [Releasing](RELEASING.md), and [Agent Instructions](AGENTS.md).

## License

[MIT](https://github.com/dblock/rust-crate-release/blob/master/LICENSE).
