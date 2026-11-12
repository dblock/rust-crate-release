#[path = "support/cli.rs"]
mod fixture;

#[test]
fn source_executable_uses_the_shared_release_fixtures() -> Result<(), Box<dyn std::error::Error>> {
    fixture::check(std::path::Path::new(env!("CARGO_BIN_EXE_rcr")))
}

#[test]
fn configured_project_runner_keeps_hooks_through_cli_and_actions()
-> Result<(), Box<dyn std::error::Error>> {
    use rust_crate_release::testing::GitProject;
    use std::{fs, path::Path, process::Command};

    let project = tempfile::tempdir()?;
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::create_dir(project.path().join("src"))?;
    fs::write(
        project.path().join("Cargo.toml"),
        format!(
            "[package]\nname = \"project-release-runner\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\nrust-crate-release = {{ path = {:?} }}\n",
            sdk
        ),
    )?;
    fs::write(
        project.path().join("src/main.rs"),
        r#"
use rust_crate_release::{Result, cli, engine::{Analyzed, Edited}, execution::ExecutionContext, hooks::LifecycleHook};
struct Hook(std::path::PathBuf);
impl LifecycleHook<Analyzed, Edited> for Hook {
    fn id(&self) -> &str { "project-version" }
    fn after(&self, output: &Edited, _: &ExecutionContext) -> Result {
        std::fs::write(self.0.join("PROJECT_VERSION"), output.release().version().to_string())?;
        Ok(())
    }
}
fn main() -> std::process::ExitCode {
    cli::main_with(|workflow| {
        workflow.hooks.prepare = workflow.hooks.prepare.with(Hook(workflow.components.package.root().into()));
        Ok(())
    })
}
"#,
    )?;
    let target = sdk.join("target/project-runner");
    let build = Command::new("cargo")
        .args(["build", "--offline", "--quiet", "--manifest-path"])
        .arg(project.path().join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .output()?;
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let binary = target.join("debug").join(format!(
        "project-release-runner{}",
        std::env::consts::EXE_SUFFIX
    ));
    let fixture_project = GitProject::cargo_library()?;
    fs::write(
        fixture_project.root().join("rcr.toml"),
        format!(
            "changelog-format = \"intridea\"\nrunner = [{}]\n",
            serde_json::to_string(binary.to_str().unwrap())?,
        ),
    )?;
    for args in [
        vec!["add", "--all"],
        vec!["commit", "-m", "Configure runner"],
        vec!["push", "origin", "master"],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(fixture_project.root())
                .status()?
                .success()
        );
    }
    let result = Command::new(env!("CARGO_BIN_EXE_rcr"))
        .current_dir(fixture_project.root())
        .env_remove("RCR_RUNNER")
        .env_remove("GITHUB_ACTIONS")
        .args(["prepare", "--save-inputs", ".git/project-inputs.json"])
        .output()?;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read_to_string(fixture_project.root().join("PROJECT_VERSION"))?,
        "0.1.0"
    );
    let result: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    assert!(
        result["archive"]
            .as_str()
            .is_some_and(|path| Path::new(path).is_file())
    );
    fixture::check(&binary)
}
