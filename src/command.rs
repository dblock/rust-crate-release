use crate::{Error, Result};
use std::path::Path;
use std::process::{Command, Output};

pub fn run(root: &Path, program: &str, args: &[&str]) -> Result<Output> {
    run_with_env(root, program, args, &[])
}

pub fn run_with_env(
    root: &Path,
    program: &str,
    args: &[&str],
    environment: &[(&str, &str)],
) -> Result<Output> {
    run_with_env_and_remove(root, program, args, environment, &[])
}

pub fn run_with_env_and_remove(
    root: &Path,
    program: &str,
    args: &[&str],
    environment: &[(&str, &str)],
    remove: &[&str],
) -> Result<Output> {
    let mut command = Command::new(program);
    command.args(args).current_dir(root);
    for name in remove {
        command.env_remove(name);
    }
    let output = command
        .env("GIT_TERMINAL_PROMPT", "0")
        .envs(environment.iter().copied())
        .output()?;
    if !output.status.success() {
        return Err(Error::invalid(format!(
            "{program} failed ({}):\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(output)
}

pub fn text(output: Output) -> Result<String> {
    String::from_utf8(output.stdout)
        .map(|value| value.trim_end_matches(['\r', '\n']).to_owned())
        .map_err(|_| Error::invalid("command output must be UTF-8"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn failed_children_preserve_both_output_channels() {
        let temporary = tempfile::tempdir().unwrap();
        let error = run(
            temporary.path(),
            "sh",
            &[
                "-c",
                "printf 'test stdout\\n'; printf 'test stderr\\n' >&2; exit 1",
            ],
        )
        .unwrap_err();
        assert!(error.to_string().contains("test stdout"));
        assert!(error.to_string().contains("test stderr"));
    }
}
