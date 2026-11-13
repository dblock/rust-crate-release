mod benchmarks;
mod docs;
mod package;
mod setup;

use std::error::Error;
use std::path::Path;
use std::process::{Command, ExitCode};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn run(root: &Path, program: &str, args: &[&str]) -> Result {
    eprintln!("> {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .status()?;
    if !status.success() {
        return Err(format!("{program} failed: {status}").into());
    }
    Ok(())
}

fn execute() -> Result {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    match std::env::args().nth(1).as_deref() {
        Some("setup") => setup::check(root)?,
        Some("bench-tests") => benchmarks::test_suites(root)?,
        Some("lint") => {
            run(root, "cargo", &["fmt", "--all", "--", "--check"])?;
            run(
                root,
                "cargo",
                &[
                    "clippy",
                    "--locked",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--",
                    "-D",
                    "warnings",
                ],
            )?;
            let status = Command::new("cargo")
                .args(["doc", "--locked", "--workspace", "--no-deps"])
                .env("RUSTDOCFLAGS", "-D warnings")
                .current_dir(root)
                .status()?;
            if !status.success() {
                return Err("rustdoc failed".into());
            }
            docs::check(root)?;
            run(root, "rumdl", &["check", "."])?;
            run(root, "typos", &[])?;
            run(root, "actionlint", &[])?;
        }
        Some("package") => package::check(root)?,
        _ => return Err("usage: cargo xtask <setup|lint|package|bench-tests>".into()),
    }
    Ok(())
}

fn main() -> ExitCode {
    match execute() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn cargo_warning_policy_rejects_warnings_but_allows_clean_builds() -> Result {
        let directory = tempfile::tempdir()?;
        fs::create_dir(directory.path().join("src"))?;
        fs::write(
            directory.path().join("Cargo.toml"),
            "[package]\nname = \"warning-probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n",
        )?;
        let source = directory.path().join("src/main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let config = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".cargo/config.toml");
        let check = || {
            Command::new("cargo")
                .args(["check", "--offline", "--quiet", "--config"])
                .arg(&config)
                .current_dir(directory.path())
                .env("CARGO_TARGET_DIR", directory.path().join("target"))
                .output()
        };
        let clean = check()?;
        assert!(
            clean.status.success(),
            "{}",
            String::from_utf8_lossy(&clean.stderr)
        );
        fs::write(&source, "fn main() { let unused = 1; }\n")?;
        let warning = check()?;
        let stderr = String::from_utf8_lossy(&warning.stderr);
        assert!(!warning.status.success(), "{stderr}");
        assert!(stderr.contains("unused variable: `unused`"), "{stderr}");
        assert!(stderr.contains("warnings are denied"), "{stderr}");
        Ok(())
    }
}
