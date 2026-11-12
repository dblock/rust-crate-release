use crate::Result;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::fs;
use std::path::Path;
use yaml_rust2::{Yaml, YamlLoader};

fn title(text: &str) -> std::result::Result<(), String> {
    let text = without_expressions(text);
    let words: Vec<_> = text
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric() && c != '.'))
        .filter(|word| !word.is_empty())
        .collect();
    for (index, word) in words.iter().enumerate() {
        if matches!(*word, "rust-crate-release" | "crates.io" | "npm" | "rcr") {
            continue;
        }
        let small = matches!(
            word.to_ascii_lowercase().as_str(),
            "a" | "an"
                | "and"
                | "as"
                | "at"
                | "but"
                | "by"
                | "for"
                | "in"
                | "nor"
                | "of"
                | "on"
                | "or"
                | "per"
                | "the"
                | "to"
                | "via"
                | "with"
        );
        if small && index > 0 && index + 1 < words.len() {
            if word.chars().any(char::is_uppercase) {
                return Err(format!("use lowercase for interior title word {word:?}"));
            }
        } else if word.chars().next().is_some_and(char::is_lowercase) {
            return Err(format!("capitalize title word {word:?}"));
        }
    }
    Ok(())
}

fn without_expressions(text: &str) -> String {
    let mut result = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${{") {
        result.push_str(&rest[..start]);
        let Some(end) = rest[start + 3..].find("}}") else {
            result.push_str(&rest[start..]);
            return result;
        };
        result.push_str("Expression");
        rest = &rest[start + 3 + end + 2..];
    }
    result.push_str(rest);
    result
}

fn markdown(source: &str) -> std::result::Result<(), String> {
    let mut heading = None;
    for event in Parser::new_ext(source, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::Heading { .. }) => heading = Some(String::new()),
            Event::End(TagEnd::Heading(_)) => title(&heading.take().unwrap())?,
            Event::Text(text) => {
                if let Some(heading) = heading.as_mut() {
                    heading.push_str(&text);
                }
            }
            Event::Code(_) => {
                if let Some(heading) = heading.as_mut() {
                    heading.push_str(" Identifier ");
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                return Err("keep each Markdown paragraph on one source line".into());
            }
            _ => {}
        }
    }
    Ok(())
}

fn display_name(node: &Yaml) -> std::result::Result<(), String> {
    match &node["name"] {
        Yaml::BadValue => Ok(()),
        Yaml::String(name) => title(name),
        _ => Err("workflow display names must be strings".into()),
    }
}

fn workflow(source: &str) -> std::result::Result<(), String> {
    let documents = YamlLoader::load_from_str(source).map_err(|error| error.to_string())?;
    if documents.len() != 1 || documents[0].as_hash().is_none() {
        return Err("expected one workflow YAML mapping".into());
    }
    let document = &documents[0];
    display_name(document)?;
    if let Some(steps) = document["runs"]["steps"].as_vec() {
        for step in steps {
            display_name(step)?;
        }
    }
    if let Some(jobs) = document["jobs"].as_hash() {
        for job in jobs.values() {
            display_name(job)?;
            if let Some(steps) = job["steps"].as_vec() {
                for step in steps {
                    display_name(step)?;
                }
            }
        }
    }
    Ok(())
}

pub fn check(root: &Path) -> Result {
    let mut errors = Vec::new();
    check_markdown_directory(root, false, &mut errors)?;
    for directory in ["docs", "examples"] {
        if root.join(directory).is_dir() {
            check_markdown_directory(&root.join(directory), true, &mut errors)?;
        }
    }
    for entry in fs::read_dir(root.join(".github/workflows"))? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "yml" || extension == "yaml")
        {
            workflow(&fs::read_to_string(&path)?)
                .unwrap_or_else(|error| errors.push(format!("{}: {error}", path.display())));
        }
        let action = root.join("action.yml");
        if action.is_file() {
            workflow(&fs::read_to_string(&action)?)
                .unwrap_or_else(|error| errors.push(format!("{}: {error}", action.display())));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n").into())
    }
}

fn check_markdown_directory(root: &Path, recursive: bool, errors: &mut Vec<String>) -> Result {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "md") {
            markdown(&fs::read_to_string(&path)?)
                .unwrap_or_else(|error| errors.push(format!("{}: {error}", path.display())));
        } else if recursive && path.is_dir() {
            check_markdown_directory(&path, true, errors)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composite_step(name: &str) -> Yaml {
        let documents = YamlLoader::load_from_str(include_str!("../../action.yml")).unwrap();
        documents[0]["runs"]["steps"]
            .as_vec()
            .unwrap()
            .iter()
            .find(|step| step["name"].as_str() == Some(name))
            .unwrap()
            .clone()
    }

    #[test]
    fn composite_action_installs_validation_components() {
        let step = composite_step("Set Up Rust");
        let components: Vec<_> = step["with"]["components"]
            .as_str()
            .expect("release validation requires Rust components")
            .split(',')
            .map(str::trim)
            .collect();
        assert!(components.contains(&"rustfmt"));
        assert!(components.contains(&"clippy"));
    }

    #[test]
    fn ci_installs_components_for_nested_release_validation() -> Result {
        for source in [
            include_str!("../../.github/workflows/test.yml"),
            include_str!("../../.github/workflows/lint.yml"),
            include_str!("../../.github/workflows/package.yml"),
        ] {
            let workflow = YamlLoader::load_from_str(source)?;
            for (id, job) in workflow[0]["jobs"].as_hash().unwrap() {
                let setup = job["steps"]
                    .as_vec()
                    .unwrap()
                    .iter()
                    .find(|step| step["name"].as_str() == Some("Set Up Rust"))
                    .unwrap();
                let components: Vec<_> = setup["with"]["components"]
                    .as_str()
                    .unwrap_or_default()
                    .split(',')
                    .map(str::trim)
                    .collect();
                for component in ["rustfmt", "clippy"] {
                    assert!(
                        components.contains(&component),
                        "{} needs {component} for fixture release validation",
                        id.as_str().unwrap()
                    );
                }
                if id.as_str() == Some("coverage") {
                    assert!(components.contains(&"llvm-tools-preview"));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn release_caches_both_cargo_workspaces_before_installation() {
        let action = YamlLoader::load_from_str(include_str!("../../action.yml")).unwrap();
        let steps = action[0]["runs"]["steps"].as_vec().unwrap();
        let index = |name| {
            steps
                .iter()
                .position(|step| step["name"].as_str() == Some(name))
                .unwrap()
        };
        assert!(index("Set Up Rust") < index("Cache Rust Dependencies"));
        assert!(index("Cache Rust Dependencies") < index("Install Release Tool"));
        let cache = composite_step("Cache Rust Dependencies");
        assert_eq!(
            cache["with"]["workspaces"]
                .as_str()
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            [
                "${{ github.action_path }} -> target",
                "${{ inputs.root }} -> target",
            ]
        );
    }

    #[test]
    fn superseded_validation_cancels_without_cancelling_releases() -> Result {
        for source in [
            include_str!("../../.github/workflows/test.yml"),
            include_str!("../../.github/workflows/lint.yml"),
            include_str!("../../.github/workflows/package.yml"),
        ] {
            let workflow = YamlLoader::load_from_str(source)?;
            assert_eq!(
                workflow[0]["concurrency"]["group"].as_str(),
                Some("${{ github.workflow }}-${{ github.ref }}")
            );
            assert_eq!(
                workflow[0]["concurrency"]["cancel-in-progress"].as_bool(),
                Some(true)
            );
        }
        let release =
            YamlLoader::load_from_str(include_str!("../../.github/workflows/release.yml"))?;
        assert_eq!(
            release[0]["concurrency"]["cancel-in-progress"].as_bool(),
            Some(false)
        );
        Ok(())
    }

    #[test]
    fn matrix_toolchain_applies_to_cache_and_child_cargo_commands() -> Result {
        let workflow = YamlLoader::load_from_str(include_str!("../../.github/workflows/test.yml"))?;
        let job = &workflow[0]["jobs"]["test"];
        assert_eq!(
            job["env"]["RUSTUP_TOOLCHAIN"].as_str(),
            Some("${{ matrix.entry.rust }}")
        );
        let steps = job["steps"].as_vec().unwrap();
        let setup = steps
            .iter()
            .find(|step| step["name"].as_str() == Some("Set Up Rust"))
            .unwrap();
        assert_eq!(setup["with"]["toolchain"], job["env"]["RUSTUP_TOOLCHAIN"]);
        for step in steps {
            assert!(
                step["env"]["RUSTUP_TOOLCHAIN"].is_badvalue(),
                "step overrides must not select another toolchain"
            );
        }
        Ok(())
    }

    #[test]
    fn setup_command_is_optional_forwarded_and_runs_before_preparation() {
        let action = YamlLoader::load_from_str(include_str!("../../action.yml")).unwrap();
        let action = &action[0];
        assert_eq!(
            action["inputs"]["setup-command"]["default"].as_str(),
            Some("")
        );
        let steps = action["runs"]["steps"].as_vec().unwrap();
        let index = |name| {
            steps
                .iter()
                .position(|step| step["name"].as_str() == Some(name))
                .unwrap()
        };
        assert!(index("Set Up Rust") < index("Set Up Project"));
        assert!(index("Install Release Tool") < index("Set Up Project"));
        assert!(index("Guard Release and Locate Diagnostics") < index("Set Up Project"));
        assert!(index("Set Up Project") < index("Prepare Release"));
        let setup = composite_step("Set Up Project");
        assert_eq!(setup["if"].as_str(), Some("inputs.setup-command != ''"));
        assert_eq!(
            setup["env"]["RCR_SETUP_COMMAND"].as_str(),
            Some("${{ inputs.setup-command }}")
        );
        assert!(setup["run"].as_str().unwrap().ends_with("actions setup"));
        let workflow =
            YamlLoader::load_from_str(include_str!("../../.github/workflows/release.yml")).unwrap();
        assert_eq!(
            workflow[0]["on"]["workflow_call"]["inputs"]["setup-command"]["default"].as_str(),
            Some("")
        );
        let release = workflow[0]["jobs"]["release"]["steps"]
            .as_vec()
            .unwrap()
            .iter()
            .find(|step| step["id"].as_str() == Some("release"))
            .unwrap();
        assert_eq!(
            release["with"]["setup-command"].as_str(),
            Some("${{ inputs.setup-command }}")
        );
    }

    #[test]
    fn self_release_and_lint_share_rust_setup() -> Result {
        let workflow =
            YamlLoader::load_from_str(include_str!("../../.github/workflows/publish.yml"))?;
        let command = workflow[0]["jobs"]["release"]["with"]["setup-command"]
            .as_str()
            .unwrap();
        let argv: Vec<String> = serde_json::from_str(command)?;
        assert_eq!(argv, ["cargo", "xtask", "setup"]);
        let lint = YamlLoader::load_from_str(include_str!("../../.github/workflows/lint.yml"))?;
        let steps = lint[0]["jobs"]["lint"]["steps"].as_vec().unwrap();
        let setup = steps
            .iter()
            .position(|step| step["run"].as_str() == Some("cargo xtask setup"))
            .unwrap();
        let validation = steps
            .iter()
            .position(|step| step["run"].as_str() == Some("cargo xtask lint"))
            .unwrap();
        assert!(
            setup < validation,
            "validation tools must be installed before lint"
        );
        Ok(())
    }

    #[test]
    fn composite_action_preserves_source_inputs_once_per_artifact() {
        for name in ["Preserve Exact Release", "Retain Final Diagnostics"] {
            let step = composite_step(name);
            assert_eq!(
                step["with"]["path"]
                    .as_str()
                    .unwrap()
                    .lines()
                    .map(str::trim)
                    .filter(
                        |path| *path == "${{ steps.paths.outputs.directory }}/source-inputs.json"
                    )
                    .count(),
                1,
                "{name} must preserve source inputs exactly once"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn composite_action_notification_preserves_status_without_executing_inputs() -> Result {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let root = tempfile::tempdir()?;
        let diagnostics = root.path().join("diagnostics");
        fs::create_dir(&diagnostics)?;
        let inputs = diagnostics.join("source-inputs.json");
        fs::write(&inputs, "{}")?;
        fs::set_permissions(&inputs, fs::Permissions::from_mode(0o600))?;
        let tool = root.path().join("rcr-tool/bin/rcr");
        fs::create_dir_all(tool.parent().unwrap())?;
        fs::write(
            &tool,
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > \"$RUNNER_TEMP/notified\"\nexit \"$RCR_TEST_EXIT\"\n",
        )?;
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755))?;
        let step = composite_step("Notify Release Result");
        let script = step["run"].as_str().unwrap().replace(
            "${{ steps.paths.outputs.directory }}",
            diagnostics.to_str().unwrap(),
        );
        for code in [0, 23] {
            let output = Command::new("bash")
                .args([
                    "--noprofile",
                    "--norc",
                    "-e",
                    "-o",
                    "pipefail",
                    "-c",
                    &script,
                ])
                .env("RUNNER_TEMP", root.path())
                .env("RCR_ROOT", root.path())
                .env("RCR_STATUS", "failure")
                .env("RCR_FAILURE_COMMAND", "")
                .env("RCR_STEPS", "{}")
                .env("RCR_TEST_EXIT", code.to_string())
                .output()?;
            assert_eq!(
                output.status.code(),
                Some(code),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                fs::read_to_string(root.path().join("notified"))?,
                format!("--root\n{}\nactions\nnotify\n", root.path().display())
            );
        }
        Ok(())
    }

    #[test]
    fn markdown_checks_titles_without_mistaking_code_for_prose() {
        assert!(markdown("# Working on rust-crate-release\n\n## Setup and Validation\n\n## `rcr prepare` and GitHub Actions\n\n```md\n# not a heading\n```\n").is_ok());
        assert!(markdown("## Setup and validation\n").is_err());
        assert!(markdown("## Setup And Validation\n").is_err());
        assert!(markdown("Setup and validation\n---\n").is_err());
    }

    #[test]
    fn markdown_rejects_wrapped_prose_but_allows_independent_blocks() {
        assert!(markdown("A paragraph\ncontinued here.\n").is_err());
        assert!(markdown("A paragraph  \ncontinued here.\n").is_err());
        assert!(markdown("- An item\n  continued here.\n").is_err());
        assert!(markdown("A paragraph.\n\nAnother paragraph.\n\n- One item.\n- Another item.\n\n```rs\nlet a = 1;\nlet b = 2;\n```\n").is_ok());
        assert!(
            markdown("| Area | Requirement |\n| --- | --- |\n| Cargo | Exact Bytes |\n").is_ok()
        );
    }

    #[test]
    fn yaml_checks_only_workflow_job_and_step_display_names() {
        assert!(workflow("name: Rust Crate Release\nruns:\n  using: composite\n  steps:\n    - name: Run Release\n      run: rcr release\n").is_ok());
        assert!(workflow("name: Test\non: push\njobs:\n  unit_tests:\n    name: Rust ${{ matrix.entry.rust }} on ${{ matrix.entry.os }}\n    runs-on: ubuntu-latest\n    steps:\n      - name: Run Tests\n        run: cargo test\n      - uses: example/action@v1\n        with:\n          name: lowercase artifact name\n").is_ok());
        for source in [
            "name: run tests\njobs: {}\n",
            "jobs:\n  test:\n    name: run Tests\n",
            "jobs:\n  test:\n    steps:\n      - name: Run tests\n",
            "name: 42\n",
            "runs:\n  using: composite\n  steps:\n    - name: Run release\n",
        ] {
            assert!(workflow(source).is_err(), "{source}");
        }
    }

    #[test]
    fn yaml_rejects_invalid_syntax_and_multiple_documents() {
        assert!(workflow("name: [\n").is_err());
        assert!(workflow("name: Test\n---\nname: Lint\n").is_err());
        assert!(workflow("- Not a mapping\n").is_err());
        assert!(workflow("jobs:\n  build:\n    steps:\n      - run: echo ok\n").is_ok());
    }

    #[test]
    fn titles_preserve_proper_names_acronyms_and_expressions() {
        for name in [
            "Build the Cargo Archive",
            "Run GitHub Actions",
            "Upload LCOV to Coveralls",
            "${{ matrix.entry.name }}",
            "Rust ${{ matrix.entry.rust }} on Windows",
        ] {
            assert!(title(name).is_ok(), "{name}");
        }
        assert_eq!(
            without_expressions("Rust ${{ matrix.entry.rust }} Tests"),
            "Rust Expression Tests"
        );
        assert_eq!(
            without_expressions("Rust ${{ unfinished"),
            "Rust ${{ unfinished"
        );
    }
}
