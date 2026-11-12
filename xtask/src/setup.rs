use crate::{Result, run};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

const CARGO_TOOLS: [(&str, &str, &str); 2] = [
    ("rumdl", "rumdl", "0.3.3"),
    ("typos", "typos-cli", "1.52.2"),
];
const ACTIONLINT_VERSION: &str = "1.7.12";

fn available(root: &Path, program: &str, flag: &str, version: &str) -> Result<bool> {
    let output = match Command::new(program).arg(flag).current_dir(root).output() {
        Ok(output) => output,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    Ok(output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .is_some_and(|line| line.split_whitespace().any(|word| word == version)))
}

fn install_actionlint(root: &Path) -> Result {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "windows",
        _ => return Err("unsupported actionlint operating system".into()),
    };
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        _ => return Err("unsupported actionlint architecture".into()),
    };
    let extension = if os == "windows" { "zip" } else { "tar.gz" };
    let asset = format!("actionlint_{ACTIONLINT_VERSION}_{os}_{architecture}.{extension}");
    let home = std::env::var_os("CARGO_HOME")
        .filter(|value| !value.is_empty())
        .map(|path| root.join(path))
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .filter(|value| !value.is_empty())
                .map(|path| PathBuf::from(path).join(".cargo"))
        })
        .ok_or("installing actionlint requires CARGO_HOME, HOME, or USERPROFILE")?;
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin)?;
    let temporary = tempfile::tempdir()?;
    let archive = temporary.path().join(&asset);
    run(
        root,
        "curl",
        &[
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            &format!(
                "https://github.com/rhysd/actionlint/releases/download/v{ACTIONLINT_VERSION}/{asset}"
            ),
            "--output",
            archive.to_str().ok_or("installer path must be UTF-8")?,
        ],
    )?;
    run(
        root,
        "tar",
        &[
            "-xf",
            archive.to_str().ok_or("installer path must be UTF-8")?,
            "-C",
            bin.to_str().ok_or("Cargo bin path must be UTF-8")?,
            &format!("actionlint{}", std::env::consts::EXE_SUFFIX),
        ],
    )
}

pub fn check(root: &Path) -> Result {
    for (program, package, version) in CARGO_TOOLS {
        if !available(root, program, "--version", version)? {
            run(
                root,
                "cargo",
                &["install", "--locked", package, "--version", version],
            )?;
            if !available(root, program, "--version", version)? {
                return Err(format!("{program} {version} is not available on PATH").into());
            }
        }
    }
    if !available(root, "actionlint", "-version", ACTIONLINT_VERSION)? {
        install_actionlint(root)?;
        if !available(root, "actionlint", "-version", ACTIONLINT_VERSION)? {
            return Err(format!("actionlint {ACTIONLINT_VERSION} is not available on PATH").into());
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn version_probe_requires_an_exact_successful_version_before_skipping_installation() -> Result {
        let directory = tempfile::tempdir()?;
        let tool = directory.path().join("validation-tool");
        for (text, code, expected) in [
            ("tool 1.2.3", 0, true),
            ("1.2.3\\nbuild metadata", 0, true),
            ("tool 1.2.30", 0, false),
            ("", 0, false),
            ("tool 1.2.3", 1, false),
        ] {
            std::fs::write(
                &tool,
                format!("#!/bin/sh\nprintf '{text}\\n'\nexit {code}\n"),
            )?;
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))?;
            assert_eq!(
                available(
                    directory.path(),
                    tool.to_str().unwrap(),
                    "--version",
                    "1.2.3"
                )?,
                expected
            );
        }
        assert!(!available(
            directory.path(),
            directory.path().join("missing").to_str().unwrap(),
            "--version",
            "1.2.3"
        )?);
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o600))?;
        assert!(
            available(
                directory.path(),
                tool.to_str().unwrap(),
                "--version",
                "1.2.3"
            )
            .is_err()
        );
        Ok(())
    }
}
