use rust_crate_release::git::{Git, ReleaseRepository};
use rust_crate_release::testing::GitProject;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn process(binary: &Path, root: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .current_dir(root)
        .env_remove("CRATE_ARCHIVE")
        .env_remove("GITHUB_ACTIONS")
        .env_remove("GITHUB_STEP_SUMMARY");
    command
}

fn success(output: Output) -> Result<Value> {
    if !output.status.success() {
        return Err(format!("CLI failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn commit(root: &Path) -> Result {
    for args in [
        vec!["add", "--all"],
        vec!["commit", "-m", "feat: configure real CLI fixture"],
        vec!["push", "origin", "master"],
    ] {
        let output = Command::new("git").args(args).current_dir(root).output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
    }
    Ok(())
}

/// Shared by the source executable and the binary installed from the actual archive.
pub fn check(binary: &Path) -> Result {
    for override_mode in ["unset", "empty", "provided"] {
        let (released, next) = if override_mode == "provided" {
            ("0.2.0", "0.2.1")
        } else {
            ("0.1.0", "0.1.1")
        };
        let fixture = GitProject::cargo_library()?;
        let root = fixture.root();
        fs::write(
            root.join("rcr.toml"),
            "changelog-format = \"intridea\"\nverification-delay-seconds = 0\n",
        )?;
        fs::write(
            root.join("src/lib.rs"),
            "pub fn answer() -> u8 {\n    42\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn development_does_not_inherit_the_released_archive() {\n        if env!(\"CARGO_PKG_VERSION\").ends_with(\".1\") {\n            assert!(std::env::var_os(\"CRATE_ARCHIVE\").is_none());\n        }\n        assert_eq!(super::answer(), 42);\n    }\n}\n",
        )?;
        commit(root)?;
        let registry = fixture.temporary_directory().join("registry");
        let mut prepare = process(binary, root);
        prepare.args(["prepare", "--save-inputs", ".git/rcr/source-inputs.json"]);
        if override_mode == "provided" {
            prepare.args(["--release-version", released]);
        }
        let prepared = success(prepare.output()?)?;
        let archive = prepared["archive"]
            .as_str()
            .ok_or("missing archive output")?;
        let bytes = fs::read(archive)?;
        assert_eq!(prepared["verification"]["status"], "pending");
        assert_eq!(prepared["operation"], "prepare");
        assert!(!registry.exists());
        let inputs = root.join(".git/rcr/source-inputs.json");
        let original_inputs = fs::read(&inputs)?;
        let mut release = process(binary, root);
        release.arg("--test-registry").arg(&registry).args([
            "release",
            "--inputs",
            ".git/rcr/source-inputs.json",
        ]);
        match override_mode {
            "empty" => {
                release.env("CRATE_ARCHIVE", "");
                release.args(["--archive", ""]);
            }
            "provided" => {
                release.env("CRATE_ARCHIVE", archive);
            }
            _ => {}
        }
        let completed = success(release.output()?)?;
        assert_eq!(completed["version"], released);
        assert_eq!(completed["next_version"], next);
        assert_eq!(completed["verification"]["status"], "verified");
        assert_eq!(fs::read(archive)?, bytes);
        assert_eq!(fs::read(&inputs)?, original_inputs);
        assert_eq!(
            fs::read(registry.join(format!("release-fixture-{released}.crate")))?,
            bytes
        );
        assert!(fs::read_to_string(root.join("Cargo.toml"))?.contains(next));
        let git = Git::new(root, "master", "origin")?;
        git.assert_clean()?;
        let head = git.inspect()?.head;
        let receipt = fs::read(root.join(".git/rcr/release.json"))?;
        let verified = success(
            process(binary, root)
                .arg("--test-registry")
                .arg(&registry)
                .args([
                    "verify",
                    "--name",
                    "release-fixture",
                    "--release-version",
                    released,
                    "--archive",
                    archive,
                ])
                .output()?,
        )?;
        assert_eq!(verified["verification"]["status"], "verified");
        assert_eq!(git.inspect()?.head, head);
        assert_eq!(fs::read(root.join(".git/rcr/release.json"))?, receipt);
        let wrong_identity = process(binary, root)
            .arg("--test-registry")
            .arg(&registry)
            .args([
                "verify",
                "--name",
                "release-fixture",
                "--release-version",
                next,
                "--archive",
                archive,
            ])
            .output()?;
        assert!(!wrong_identity.status.success());
        assert!(wrong_identity.stdout.is_empty());
        let retry = success(
            process(binary, root)
                .arg("--test-registry")
                .arg(&registry)
                .args(["release", "--inputs", ".git/rcr/source-inputs.json"])
                .output()?,
        )?;
        assert_eq!(retry["checksum"], completed["checksum"]);
        assert_eq!(git.inspect()?.head, head);
    }
    let fixture = GitProject::cargo_library()?;
    for config in [
        "unknown-option = true\n",
        "workflow = \"semantic\"\nchangelog-format = \"intridea\"\n",
        "validation = []\n",
        "verification-attempts = 0\n",
        "timezone = \"not-a-zone\"\n",
    ] {
        let path = fixture.temporary_directory().join("invalid.toml");
        fs::write(&path, config)?;
        let output = process(binary, fixture.root())
            .arg("--config")
            .arg(path)
            .arg("prepare")
            .output()?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        Git::new(fixture.root(), "master", "origin")?.assert_clean()?;
    }
    let flags = process(binary, fixture.root())
        .args(["verify", "--save-inputs", "invalid"])
        .output()?;
    assert_eq!(flags.status.code(), Some(2));
    let version = process(binary, fixture.root()).arg("--version").output()?;
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("rcr "));
    let semantic = GitProject::cargo_library()?;
    fs::remove_file(semantic.root().join("CHANGELOG.md"))?;
    fs::write(
        semantic.root().join("rcr.toml"),
        "workflow = \"semantic\"\nverification-delay-seconds = 0\n",
    )?;
    commit(semantic.root())?;
    let prepared = success(
        process(binary, semantic.root())
            .args(["prepare", "--save-inputs", ".git/rcr/source-inputs.json"])
            .output()?,
    )?;
    assert_eq!(prepared["version"], "1.0.0");
    let completed = success(
        process(binary, semantic.root())
            .arg("--test-registry")
            .arg(semantic.temporary_directory().join("registry"))
            .args(["release", "--inputs", ".git/rcr/source-inputs.json"])
            .output()?,
    )?;
    assert_eq!(completed["version"], "1.0.0");
    assert!(completed["next_version"].is_null());
    assert_eq!(
        completed["captured"]["semantic"],
        prepared["captured"]["semantic"]
    );
    let binding_config = fixture.temporary_directory().join("binding.toml");
    fs::write(&binding_config, "changelog-format = \"intridea\"\n")?;
    let github_process = |attempt: &str| {
        let mut command = process(binary, fixture.root());
        command
            .arg("--config")
            .arg(&binding_config)
            .env("GITHUB_ACTIONS", "true")
            .env("GITHUB_EVENT_NAME", "workflow_dispatch")
            .env("GITHUB_REF", "refs/heads/master")
            .env("GITHUB_REPOSITORY", "fixture/project")
            .env("GITHUB_RUN_ID", "42")
            .env("GITHUB_RUN_ATTEMPT", attempt)
            .env(
                "GITHUB_WORKFLOW_REF",
                "fixture/project/.github/workflows/release.yml@refs/heads/master",
            )
            .env(
                "GITHUB_STEP_SUMMARY",
                fixture.temporary_directory().join("summary"),
            );
        command
    };
    success(
        github_process("1")
            .args([
                "prepare",
                "--save-inputs",
                ".git/rcr/github-source-inputs.json",
            ])
            .output()?,
    )?;
    let wrong_attempt = github_process("2")
        .args(["prepare", "--inputs", ".git/rcr/github-source-inputs.json"])
        .output()?;
    assert!(!wrong_attempt.status.success());
    assert!(
        String::from_utf8_lossy(&wrong_attempt.stderr).contains("another GitHub run or attempt")
    );
    Ok(())
}
