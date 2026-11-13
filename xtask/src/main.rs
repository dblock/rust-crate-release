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
