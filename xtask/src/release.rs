use regex::Regex;
use rust_crate_release::{
    Error, Result,
    engine::{Hosted, NextEdited, ReleaseWorkflow},
    execution::ExecutionContext,
    hooks::LifecycleHook,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

const DOCUMENTS: [&str; 4] = [
    "README.md",
    "docs/actions.md",
    "docs/cli.md",
    "docs/releases.md",
];

pub fn configure(workflow: &mut ReleaseWorkflow) -> Result {
    workflow.hooks.next = workflow
        .hooks
        .next
        .with(DocumentationPins(workflow.components.package.root().into()));
    Ok(())
}

struct DocumentationPins(PathBuf);

impl LifecycleHook<Hosted, NextEdited> for DocumentationPins {
    fn id(&self) -> &str {
        "released-documentation-pins"
    }

    fn after(&self, output: &NextEdited, _: &ExecutionContext) -> Result {
        if output.next_version().is_some() {
            update_documents(&self.0, &output.release().version().to_string())?;
        }
        Ok(())
    }
}

fn update_pins(source: &str, version: &str) -> Result<String> {
    let mut updated = source.to_owned();
    for pattern in [
        r"(dblock/rust-crate-release(?:/\.github/workflows/release\.yml)?@v)\d+\.\d+\.\d+",
        r"(?m)^([ \t]*tool-ref:[ \t]*v)\d+\.\d+\.\d+",
        r"(cargo install --locked rust-crate-release --version )\d+\.\d+\.\d+",
        r"(cargo add --dev rust-crate-release@)\d+\.\d+\.\d+",
        r"(rust-crate-release `v)\d+\.\d+\.\d+",
    ] {
        let matcher =
            Regex::new(pattern).map_err(|_| Error::invalid("invalid documentation pin pattern"))?;
        updated = matcher
            .replace_all(&updated, format!("${{1}}{version}"))
            .into_owned();
    }
    Ok(updated)
}

fn update_documents(root: &Path, version: &str) -> Result {
    let changes: Vec<_> = DOCUMENTS
        .into_iter()
        .map(|name| {
            let path = root.join(name);
            if !fs::symlink_metadata(&path)?.is_file() {
                return Err(Error::invalid("documentation pins require regular files"));
            }
            let source = fs::read_to_string(&path)?;
            let updated = update_pins(&source, version)?;
            Ok((path, source, updated))
        })
        .collect::<Result<_>>()?;
    for (path, source, updated) in changes {
        if source != updated {
            fs::write(path, updated)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_crate_release::{
        git::{Git, ReleaseRepository},
        testing::{GitProject, ReleaseFixture},
    };

    #[test]
    fn pin_updates_preserve_examples_line_endings_and_validate_all_files_before_writing() -> Result
    {
        let source = "uses: dblock/rust-crate-release/.github/workflows/release.yml@v0.1.0\r\n  tool-ref: v0.1.0\r\ncargo install --locked rust-crate-release --version 0.1.0\r\ncargo add --dev rust-crate-release@0.1.0\r\nExample crate 0.1.0\r\nuses: acme/other@v0.1.0\r\n";
        let updated = update_pins(source, "0.2.0")?;
        assert_eq!(updated.matches("0.2.0").count(), 4);
        assert!(updated.ends_with("Example crate 0.1.0\r\nuses: acme/other@v0.1.0\r\n"));
        assert_eq!(
            updated.matches("\r\n").count(),
            source.matches("\r\n").count()
        );
        assert_eq!(update_pins(&updated, "0.2.0")?, updated);
        let directory = tempfile::tempdir()?;
        fs::create_dir(directory.path().join("docs"))?;
        for name in DOCUMENTS {
            fs::write(directory.path().join(name), source)?;
        }
        fs::write(directory.path().join("docs/releases.md"), [0xff])?;
        assert!(update_documents(directory.path(), "0.2.0").is_err());
        assert_eq!(
            fs::read_to_string(directory.path().join("README.md"))?,
            source
        );
        Ok(())
    }

    #[test]
    fn self_release_updates_pins_only_in_validated_next_development_commit() -> Result {
        let project = GitProject::cargo_library()?;
        fs::create_dir(project.root().join("docs"))?;
        let source = "cargo install --locked rust-crate-release --version 0.1.0\n";
        for name in DOCUMENTS {
            fs::write(project.root().join(name), source)?;
        }
        fs::write(
            project.root().join("rcr.toml"),
            "changelog-format = \"intridea\"\n",
        )?;
        let git = Git::new(project.root(), "master", "origin")?;
        let tree = git.stage_changes()?;
        git.commit(&tree, "Add documentation fixtures")?;
        assert!(
            std::process::Command::new("git")
                .args(["push", "origin", "master"])
                .current_dir(project.root())
                .status()?
                .success()
        );
        let mut fixture = ReleaseFixture::new(project)?;
        fixture.configure_workflow(configure);
        let output = fixture.release(Some("0.2.0"))?;
        assert_eq!(output["verification"]["status"], "verified");
        for name in DOCUMENTS {
            assert_eq!(
                fixture.remote_file("v0.2.0", Path::new(name))?,
                source.as_bytes()
            );
            assert_eq!(
                fixture.remote_file("master", Path::new(name))?,
                b"cargo install --locked rust-crate-release --version 0.2.0\n"
            );
        }
        assert!(
            String::from_utf8(fixture.remote_file("master", Path::new("Cargo.toml"))?)
                .map_err(|_| Error::invalid("non-UTF-8 manifest"))?
                .contains("0.2.1")
        );
        assert_eq!(fixture.registry.publication_count(), 1);
        assert_eq!(fixture.host.publication_count(), 1);
        Ok(())
    }
}
