use crate::{Result, run};
use sha2::{Digest, Sha256};
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

fn cargo_bin(root: &Path) -> Result<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .filter(|value| !value.is_empty())
        .map(|path| root.join(path))
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .filter(|value| !value.is_empty())
                .map(|path| PathBuf::from(path).join(".cargo"))
        })
        .ok_or("installing validation tools requires CARGO_HOME, HOME, or USERPROFILE")?;
    Ok(home.join("bin"))
}

fn archive_checksum(archive: &Path) -> Result<String> {
    Ok(Sha256::digest(std::fs::read(archive)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn extract_binary(
    root: &Path,
    archive: &Path,
    bin: &Path,
    program: &str,
    checksum: Option<&str>,
) -> Result {
    if let Some(expected) = checksum
        && archive_checksum(archive)? != expected
    {
        return Err(format!("{program} download checksum mismatch").into());
    }
    std::fs::create_dir_all(bin)?;
    run(
        root,
        "tar",
        &[
            "-xf",
            archive.to_str().ok_or("installer path must be UTF-8")?,
            "-C",
            bin.to_str().ok_or("Cargo bin path must be UTF-8")?,
            &format!("{program}{}", std::env::consts::EXE_SUFFIX),
        ],
    )
}

fn install_binary(
    root: &Path,
    repository: &str,
    version: &str,
    asset: &str,
    program: &str,
    checksum: Option<&str>,
) -> Result {
    let temporary = tempfile::tempdir()?;
    let archive = temporary.path().join(asset);
    run(
        root,
        "curl",
        &[
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            &format!("https://github.com/{repository}/releases/download/v{version}/{asset}"),
            "--output",
            archive.to_str().ok_or("installer path must be UTF-8")?,
        ],
    )?;
    extract_binary(root, &archive, &cargo_bin(root)?, program, checksum)
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
    install_binary(
        root,
        "rhysd/actionlint",
        ACTIONLINT_VERSION,
        &format!("actionlint_{ACTIONLINT_VERSION}_{os}_{architecture}.{extension}"),
        "actionlint",
        None,
    )
}

fn install_cargo_tool(root: &Path, program: &str, package: &str, version: &str) -> Result {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        let (repository, checksum) = match package {
            "rumdl" => (
                "rvben/rumdl",
                "225291d073d46654030cb2c256ce98252965e1f475b77fee4f28a3dbb3632cfd",
            ),
            "typos-cli" => (
                "crate-ci/typos",
                "3b13d1291281d39d01822e54035636487a7a0f228e70097899b1e33aeb0e7e93",
            ),
            _ => return Err(format!("unknown validation tool: {package}").into()),
        };
        install_binary(
            root,
            repository,
            version,
            &format!("{program}-v{version}-x86_64-unknown-linux-musl.tar.gz"),
            if program == "typos" {
                "./typos"
            } else {
                program
            },
            Some(checksum),
        )
    } else {
        run(
            root,
            "cargo",
            &["install", "--locked", package, "--version", version],
        )
    }
}

pub fn check(root: &Path) -> Result {
    for (program, package, version) in CARGO_TOOLS {
        if !available(root, program, "--version", version)? {
            install_cargo_tool(root, program, package, version)?;
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
    fn binary_installation_checks_bytes_before_replacing_a_tool() -> Result {
        let directory = tempfile::tempdir()?;
        let archive = directory.path().join("tool.tar.gz");
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            std::fs::File::create(&archive)?,
            flate2::Compression::default(),
        ));
        let script = b"#!/bin/sh\nprintf 'tool 1.2.3\\n'\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(script.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, "tool", script.as_slice())?;
        builder.into_inner()?.finish()?;
        let checksum = archive_checksum(&archive)?;
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin)?;
        std::fs::write(bin.join("tool"), "existing tool")?;
        assert!(
            extract_binary(directory.path(), &archive, &bin, "tool", Some("wrong"))
                .unwrap_err()
                .to_string()
                .contains("checksum mismatch")
        );
        assert_eq!(std::fs::read_to_string(bin.join("tool"))?, "existing tool");
        extract_binary(directory.path(), &archive, &bin, "tool", Some(&checksum))?;
        assert!(available(
            directory.path(),
            bin.join("tool").to_str().unwrap(),
            "--version",
            "1.2.3"
        )?);
        assert!(
            extract_binary(directory.path(), &archive, &bin, "missing", Some(&checksum)).is_err()
        );
        Ok(())
    }

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
