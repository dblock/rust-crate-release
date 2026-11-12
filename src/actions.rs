//! Durable GitHub artifact preservation through the official Actions client.

use crate::engine::ArtifactPreserver;
use crate::execution::ExecutionProvider;
use crate::package::Artifact;
use crate::{Error, Release, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub struct GitHubArtifacts {
    helper: PathBuf,
}

impl GitHubArtifacts {
    /// The helper is trusted executable code, supplied by the reviewed action.
    pub fn new(helper: impl Into<PathBuf>) -> Result<Self> {
        let helper = dunce::canonicalize(helper.into())?;
        if !helper.is_file() {
            return Err(Error::invalid(
                "artifact uploader must be a regular helper file",
            ));
        }
        Ok(Self { helper })
    }
}

impl ArtifactPreserver for GitHubArtifacts {
    fn preserve(
        &self,
        release: &Release,
        directory: &Path,
        execution: &dyn ExecutionProvider,
    ) -> Result {
        if execution.metadata().platform != "github" {
            return Err(Error::invalid(
                "GitHub artifact uploader requires GitHub execution",
            ));
        }
        let archive = Artifact::open(&directory.join(format!(
            "{}-{}.crate",
            release.name(),
            release.version()
        )))?;
        if archive.name() != release.name() || archive.version() != release.version() {
            return Err(Error::invalid(
                "preserved archive differs from release identity",
            ));
        }
        let output = Command::new("node")
            .arg(&self.helper)
            .arg(directory)
            .arg(release.name())
            .arg(release.version().to_string())
            .arg(archive.checksum())
            .output()?;
        if !output.status.success() {
            return Err(Error::invalid(
                "durable GitHub artifact upload failed; publication was not attempted",
            ));
        }
        archive.verify()?;
        eprintln!("Durably preserved release artifacts in GitHub Actions.");
        Ok(())
    }
}
