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
pub fn check(binary: &Path, support: &Path) -> Result {
    for override_mode in ["unset", "empty", "provided"] {
        let (released, next) = if override_mode == "provided" {
            ("0.2.0", "0.2.1")
        } else {
            ("0.1.0", "0.1.1")
        };
        let storage = if override_mode == "provided" {
            ".git/cli-rcr"
        } else {
            ".git/rcr"
        };
        let fixture = GitProject::cargo_library()?;
        let root = fixture.root();
        fs::write(
            root.join("rcr.toml"),
            "changelog-format = \"intridea\"\nverification-delay-seconds = 0\nartifact-directory = \".git/configured-rcr\"\n",
        )?;
        fs::write(
            root.join("src/lib.rs"),
            "pub fn answer() -> u8 {\n    42\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn development_does_not_inherit_the_released_archive() {\n        if env!(\"CARGO_PKG_VERSION\").ends_with(\".1\") {\n            assert!(std::env::var_os(\"CRATE_ARCHIVE\").is_none());\n        }\n        assert_eq!(super::answer(), 42);\n    }\n}\n",
        )?;
        commit(root)?;
        let registry = fixture.temporary_directory().join("registry");
        let mut prepare = process(binary, root);
        prepare.args(["--artifact-directory", storage]);
        prepare.args(["prepare", "--save-inputs", ".git/rcr/source-inputs.json"]);
        if override_mode == "provided" {
            prepare.args(["--release-version", released]);
        }
        let prepared = success(prepare.output()?)?;
        let archive = prepared["archive"]
            .as_str()
            .ok_or("missing archive output")?;
        assert!(Path::new(archive).parent().unwrap().ends_with(storage));
        let bytes = fs::read(archive)?;
        assert_eq!(prepared["verification"]["status"], "pending");
        assert_eq!(prepared["operation"], "prepare");
        assert!(!registry.exists());
        let inputs = root.join(".git/rcr/source-inputs.json");
        let original_inputs = fs::read(&inputs)?;
        let mut release = process(binary, root);
        release.args(["--artifact-directory", storage]);
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
        let receipt_path = root.join(storage).join("release.json");
        let receipt = fs::read(&receipt_path)?;
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
        assert_eq!(fs::read(&receipt_path)?, receipt);
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
                .args(["--artifact-directory", storage])
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
    let registry = fixture.temporary_directory().join("github-registry");
    let helper = fixture.temporary_directory().join("uploader.mjs");
    fs::write(
        &helper,
        "console.error('dummy uploader detail must not leak'); process.exit(1);\n",
    )?;
    let upload_failure = github_process("1")
        .arg("--test-registry")
        .arg(&registry)
        .arg("--github-artifact-uploader")
        .arg(&helper)
        .args(["release", "--inputs", ".git/rcr/github-source-inputs.json"])
        .output()?;
    assert!(!upload_failure.status.success());
    assert!(upload_failure.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&upload_failure.stderr)
            .contains("durable GitHub artifact upload failed")
    );
    assert!(!String::from_utf8_lossy(&upload_failure.stderr).contains("dummy uploader detail"));
    assert_eq!(fs::read_dir(&registry)?.count(), 0);
    let marker = fixture.temporary_directory().join("preserved.json");
    fs::write(
        &helper,
        format!(
            "import fs from 'node:fs';\nconst args = process.argv.slice(2);\n\
         if (fs.existsSync({registry}) || args.length !== 4 || !fs.existsSync(args[0] + '/' + args[1] + '-' + args[2] + '.bundle')) process.exit(2);\n\
         fs.writeFileSync({marker}, JSON.stringify(args));\n",
            registry = serde_json::to_string(&registry.join("release-fixture-0.1.0.crate"))?,
            marker = serde_json::to_string(&marker)?,
        ),
    )?;
    let completed = success(
        github_process("1")
            .arg("--test-registry")
            .arg(&registry)
            .arg("--github-artifact-uploader")
            .arg(&helper)
            .args(["release", "--inputs", ".git/rcr/github-source-inputs.json"])
            .output()?,
    )?;
    assert_eq!(completed["verification"]["status"], "verified");
    let preserved: Value = serde_json::from_slice(&fs::read(marker)?)?;
    assert_eq!(preserved[1], "release-fixture");
    assert_eq!(preserved[2], "0.1.0");
    assert_eq!(preserved[3], completed["checksum"]);
    check_action(binary, support)
}

fn check_action(binary: &Path, support: &Path) -> Result {
    let fixture = GitProject::cargo_library()?;
    fs::write(
        fixture.root().join("rcr.toml"),
        "changelog-format = \"intridea\"\nverification-delay-seconds = 0\n",
    )?;
    commit(fixture.root())?;
    let temporary = fixture.temporary_directory();
    let registry = temporary.join("action-registry");
    let helper = temporary.join("action-uploader.mjs");
    let marker = temporary.join("preservation.json");
    fs::write(
        &helper,
        format!(
            "import fs from 'node:fs';\nif (fs.existsSync({registry})) process.exit(1);\nfs.writeFileSync({marker}, JSON.stringify(process.argv.slice(2)));\n",
            registry = serde_json::to_string(&registry.join("release-fixture-0.1.0.crate"))?,
            marker = serde_json::to_string(&marker)?,
        ),
    )?;
    let harness = temporary.join("action-harness.mjs");
    fs::write(
        &harness,
        format!(
            "import {{ pathToFileURL }} from 'node:url';\n\
         const binary = {binary};\nconst {{runAction, runCommand}} = await import(pathToFileURL({runner}).href);\n\
         const execute = (program, args, cwd, json = false) => {{\n\
           if (program === binary) args = ['--test-registry', {registry}, ...args];\n\
           else if (program !== 'git') throw new Error('Unexpected fixture command');\n\
           return runCommand(program, args, cwd, json);\n\
         }};\nrunAction('guard', process.env, execute);\nrunAction('release', process.env, execute);\n",
            binary = serde_json::to_string(binary)?,
            runner = serde_json::to_string(&support.join("runner.mjs"))?,
            registry = serde_json::to_string(&registry)?,
        ),
    )?;
    let output_path = temporary.join("action-output");
    let output = process(Path::new("node"), fixture.root())
        .arg(harness)
        .env("GITHUB_ACTIONS", "true")
        .env("GITHUB_SERVER_URL", "https://github.com")
        .env("GITHUB_EVENT_NAME", "workflow_dispatch")
        .env("GITHUB_REF", "refs/heads/master")
        .env("GITHUB_REPOSITORY", "fixture/project")
        .env("GITHUB_RUN_ID", "42")
        .env("GITHUB_RUN_ATTEMPT", "1")
        .env(
            "GITHUB_WORKFLOW_REF",
            "fixture/project/.github/workflows/release.yml@refs/heads/master",
        )
        .env("RUNNER_ENVIRONMENT", "github-hosted")
        .env("RCR_DEFAULT_BRANCH", "master")
        .env("RCR_ROOT", fixture.root())
        .env("RCR_BINARY", binary)
        .env("RCR_HELPER", helper)
        .env_remove("RCR_CONFIG")
        .env_remove("RCR_VERSION")
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env("GITHUB_OUTPUT", &output_path)
        .env("GITHUB_STEP_SUMMARY", temporary.join("summary"))
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "Actual action runner failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let outputs = fs::read_to_string(output_path)?;
    assert!(outputs.contains("version=0.1.0\n"));
    assert!(outputs.contains("\"status\":\"verified\""));
    let preserved: Value = serde_json::from_slice(&fs::read(marker)?)?;
    assert_eq!(preserved[1], "release-fixture");
    assert_eq!(
        fs::read(registry.join("release-fixture-0.1.0.crate"))?,
        fs::read(fixture.root().join(".git/rcr/release-fixture-0.1.0.crate"))?
    );
    Git::new(fixture.root(), "master", "origin")?.assert_clean()?;
    Ok(())
}
