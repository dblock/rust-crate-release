use crate::{Result, run};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path};
use std::process::Command;
use tar::Archive;

#[path = "../../tests/support/cli.rs"]
mod cli_fixture;

fn expected_file(path: &str) -> bool {
    (path.starts_with("src/") && path.ends_with(".rs"))
        || (path.starts_with("docs/") && path.ends_with(".md"))
        || matches!(
            path,
            ".cargo_vcs_info.json"
                | "Cargo.toml"
                | "Cargo.toml.orig"
                | "Cargo.lock"
                | "README.md"
                | "LICENSE"
                | "CHANGELOG.md"
                | "DESIGN.md"
                | "CONTRIBUTING.md"
                | "AGENTS.md"
                | "RELEASING.md"
                | "action.yml"
                | ".github/workflows/release.yml"
                | "assets/rust-crate-release.svg"
                | "assets/rust-crate-release.png"
        )
}

pub fn check(root: &Path) -> Result {
    let metadata = Command::new("cargo")
        .args(["metadata", "--locked", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()?;
    if !metadata.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&metadata.stderr)
        )
        .into());
    }
    let metadata: Value = serde_json::from_slice(&metadata.stdout)?;
    let package = metadata["packages"]
        .as_array()
        .ok_or("missing Cargo packages")?
        .iter()
        .find(|package| package["name"] == "rust-crate-release")
        .ok_or("missing release package")?;
    let version = package["version"].as_str().ok_or("missing Cargo version")?;
    let archive_root = format!("rust-crate-release-{version}");
    let target = metadata["target_directory"]
        .as_str()
        .ok_or("missing target directory")?;
    run(
        root,
        "cargo",
        &[
            "package",
            "--locked",
            "--allow-dirty",
            "--package",
            "rust-crate-release",
        ],
    )?;
    let archive_path = Path::new(target)
        .join("package")
        .join(format!("{archive_root}.crate"));
    let bytes = fs::read(&archive_path)?;
    let temporary = tempfile::tempdir()?;
    let mut archive = Archive::new(GzDecoder::new(bytes.as_slice()));
    let mut files = BTreeSet::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let relative = path.strip_prefix(&archive_root)?;
        if !entry.header().entry_type().is_file()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(format!("unexpected archive entry: {}", path.display()).into());
        }
        let relative = relative.to_str().ok_or("non-UTF-8 archive path")?;
        if !expected_file(relative) || !files.insert(relative.to_owned()) {
            return Err(format!("unexpected or duplicate archive file: {relative}").into());
        }
        if !entry.unpack_in(temporary.path())? {
            return Err("archive extraction escaped the consumer directory".into());
        }
    }
    for required in [
        "src/lib.rs",
        "Cargo.toml",
        "Cargo.lock",
        "README.md",
        "LICENSE",
        "assets/rust-crate-release.svg",
        "assets/rust-crate-release.png",
    ] {
        if !files.contains(required) {
            return Err(format!("missing archive file: {required}").into());
        }
    }
    let consumer = temporary.path().join("consumer");
    fs::create_dir_all(consumer.join("src"))?;
    fs::write(
        consumer.join("Cargo.toml"),
        format!(
            "[package]\nname = \"archive-consumer\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
             [dependencies]\nrust-crate-release = {{ path = \"../{archive_root}\", version = \"={version}\" }}\n"
        ),
    )?;
    fs::write(
        consumer.join("src/main.rs"),
        "use rust_crate_release::{execution::ExecutionContext, Result, Step, Workflow};\n\
         struct Length;\nimpl Step<String> for Length {\n\
         type Output = usize;\nfn id(&self) -> &str { \"length\" }\n\
         fn run(&self, value: String, _: &ExecutionContext) -> Result<usize> { Ok(value.len()) }\n}\n\
         fn main() -> Result { assert_eq!(Workflow::start().then(Length)?.run(\"crate\".into())?, 5); Ok(()) }\n",
    )?;
    run(&consumer, "cargo", &["generate-lockfile", "--offline"])?;
    run(&consumer, "cargo", &["run", "--locked", "--offline"])?;
    let install = temporary.path().join("installed");
    run(
        temporary.path(),
        "cargo",
        &[
            "install",
            "--locked",
            "--offline",
            "--path",
            temporary
                .path()
                .join(&archive_root)
                .to_str()
                .ok_or("non-UTF-8 extracted path")?,
            "--root",
            install.to_str().ok_or("non-UTF-8 installation path")?,
            "--debug",
            "--bin",
            "rcr",
        ],
    )?;
    cli_fixture::check(
        &install
            .join("bin")
            .join(format!("rcr{}", std::env::consts::EXE_SUFFIX)),
    )?;
    if fs::read(&archive_path)? != bytes {
        return Err("validated archive changed during consumer validation".into());
    }
    eprintln!(
        "Validated {} ({} bytes, {} files) and its extracted-artifact consumer.",
        archive_path.display(),
        bytes.len(),
        files.len()
    );
    Ok(())
}
