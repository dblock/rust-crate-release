use crate::package::Artifact;
use crate::{Error, Release, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Capture,
    Analyze,
    Prepare,
    Validate,
    Commit,
    Build,
    Preserve,
    Publish,
    Push,
    Host,
    Next,
    Verify,
    Complete,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Analyze => "analyze",
            Self::Prepare => "prepare",
            Self::Validate => "validate",
            Self::Commit => "commit",
            Self::Build => "build",
            Self::Preserve => "preserve",
            Self::Publish => "publish",
            Self::Push => "push",
            Self::Host => "host",
            Self::Next => "next",
            Self::Verify => "verify",
            Self::Complete => "complete",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Verification {
    #[default]
    Pending,
    Verified {
        checksum: String,
    },
    NotVisible {
        attempts: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseIdentity {
    pub name: String,
    pub version: Version,
    pub manifest_version: Version,
    pub source_commit: String,
    pub date: String,
    pub tag: String,
}

impl From<&Release> for ReleaseIdentity {
    fn from(release: &Release) -> Self {
        Self {
            name: release.name().into(),
            version: release.version().clone(),
            manifest_version: release.manifest_version().clone(),
            source_commit: release.source_commit().into(),
            date: release.date().into(),
            tag: release.tag().into(),
        }
    }
}

impl ReleaseIdentity {
    pub fn release(&self) -> Result<Release> {
        let release = Release::new(
            &self.name,
            self.version.clone(),
            self.manifest_version.clone(),
            &self.source_commit,
            &self.date,
        )?;
        if self.tag != release.tag() {
            return Err(Error::invalid(
                "diagnostic tag conflicts with release version",
            ));
        }
        Ok(release)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRecord {
    pub filename: String,
    pub checksum: String,
    pub size: u64,
}

/// Diagnostic evidence only. No phase value authorizes skipping an operation.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseReceipt {
    pub release: Option<ReleaseIdentity>,
    pub phase: Phase,
    pub source_date_epoch: Option<i64>,
    pub remote_source: Option<String>,
    pub prepared_tree: Option<String>,
    pub release_commit: Option<String>,
    pub artifact: Option<ArtifactRecord>,
    pub next_version: Option<Version>,
    pub development_commit: Option<String>,
    pub verification: Verification,
    pub error: Option<String>,
}

fn object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|c| c.is_ascii_hexdigit())
}

fn checksum(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}

impl ReleaseReceipt {
    pub fn capture(release: &Release, remote_source: &str, timestamp: i64) -> Self {
        Self {
            release: Some(release.into()),
            remote_source: Some(remote_source.into()),
            source_date_epoch: Some(timestamp),
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result {
        if let Some(identity) = &self.release {
            identity.release()?;
        } else if self.artifact.is_some()
            || self.release_commit.is_some()
            || self.prepared_tree.is_some()
            || self.next_version.is_some()
            || self.development_commit.is_some()
            || self.verification != Verification::Pending
        {
            return Err(Error::invalid(
                "diagnostic outputs require a captured release identity",
            ));
        }
        for value in [
            &self.remote_source,
            &self.prepared_tree,
            &self.release_commit,
            &self.development_commit,
        ]
        .into_iter()
        .flatten()
        {
            if !object_id(value) {
                return Err(Error::invalid("invalid diagnostic Git object ID"));
            }
        }
        if self.source_date_epoch.is_some_and(|value| value < 0) {
            return Err(Error::invalid("invalid diagnostic source timestamp"));
        }
        if let Some(next) = &self.next_version {
            let release = self.release.as_ref().unwrap();
            if next <= &release.version || !next.pre.is_empty() || !next.build.is_empty() {
                return Err(Error::invalid(
                    "invalid next-development diagnostic version",
                ));
            }
        }
        if let Some(artifact) = &self.artifact {
            let identity = self.release.as_ref().unwrap();
            if artifact.filename != format!("{}-{}.crate", identity.name, identity.version)
                || !checksum(&artifact.checksum)
                || artifact.size == 0
            {
                return Err(Error::invalid("invalid diagnostic archive identity"));
            }
        }
        if let Verification::Verified { checksum: verified } = &self.verification
            && self.artifact.as_ref().map(|value| &value.checksum) != Some(verified)
        {
            return Err(Error::invalid(
                "verification diagnostic does not match the retained archive",
            ));
        }
        Ok(())
    }

    pub fn record_artifact(&mut self, artifact: &Artifact) -> Result {
        let identity = self
            .release
            .as_ref()
            .ok_or_else(|| Error::invalid("release identity was not captured"))?;
        if artifact.name() != identity.name || artifact.version() != &identity.version {
            return Err(Error::invalid(
                "artifact does not match the captured release",
            ));
        }
        artifact.verify()?;
        self.artifact = Some(ArtifactRecord {
            filename: format!("{}-{}.crate", artifact.name(), artifact.version()),
            checksum: artifact.checksum().into(),
            size: fs::metadata(artifact.path())?.len(),
        });
        Ok(())
    }
}

pub trait ReleaseStore: Send + Sync {
    fn directory(&self) -> &Path;
    fn write(&self, receipt: &ReleaseReceipt) -> Result;
    fn read(&self) -> Result<Option<ReleaseReceipt>>;
}

#[derive(Clone, Debug)]
pub struct DirectoryReleaseStore {
    directory: PathBuf,
}

impl DirectoryReleaseStore {
    pub fn new(directory: impl Into<PathBuf>) -> Result<Self> {
        let directory = directory.into();
        fs::create_dir_all(&directory)?;
        Ok(Self {
            directory: dunce::canonicalize(directory)?,
        })
    }

    fn record_path(&self) -> Result<PathBuf> {
        let path = self.directory.join("release.json");
        if let Ok(metadata) = fs::symlink_metadata(&path)
            && !metadata.is_file()
        {
            return Err(Error::invalid("diagnostic path must be a regular file"));
        }
        Ok(path)
    }

    pub fn retained_artifact(&self, receipt: &ReleaseReceipt) -> Result<Artifact> {
        receipt.validate()?;
        let record = receipt
            .artifact
            .as_ref()
            .ok_or_else(|| Error::invalid("no retained release artifact"))?;
        let path = self.directory.join(&record.filename);
        if !fs::symlink_metadata(&path)?.is_file() {
            return Err(Error::invalid("retained archive must be a regular file"));
        }
        let artifact = Artifact::open(&path)?;
        let identity = receipt.release.as_ref().unwrap();
        if artifact.name() != identity.name
            || artifact.version() != &identity.version
            || artifact.checksum() != record.checksum
            || fs::metadata(&path)?.len() != record.size
        {
            return Err(Error::invalid(
                "retained archive differs from release diagnostics",
            ));
        }
        Ok(artifact)
    }
}

impl ReleaseStore for DirectoryReleaseStore {
    fn directory(&self) -> &Path {
        &self.directory
    }

    fn write(&self, receipt: &ReleaseReceipt) -> Result {
        receipt.validate()?;
        let path = self.record_path()?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        serde_json::to_writer_pretty(&mut temporary, receipt)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    fn read(&self) -> Result<Option<ReleaseReceipt>> {
        let path = self.record_path()?;
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(Error::invalid(
                "release diagnostics exceed the supported size",
            ));
        }
        let receipt: ReleaseReceipt = serde_json::from_slice(&bytes)?;
        receipt.validate()?;
        Ok(Some(receipt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt() -> Result<ReleaseReceipt> {
        let release = Release::new(
            "fixture",
            Version::new(0, 1, 0),
            Version::new(0, 1, 0),
            "a".repeat(40),
            "2026/11/12",
        )?;
        Ok(ReleaseReceipt::capture(
            &release,
            &"a".repeat(40),
            1_700_000_000,
        ))
    }

    #[test]
    fn diagnostics_are_atomic_validated_and_not_a_skip_protocol() -> Result {
        let temporary = tempfile::tempdir()?;
        let store = DirectoryReleaseStore::new(temporary.path())?;
        assert_eq!(store.read()?, None);
        let original = receipt()?;
        store.write(&original)?;
        let bytes = fs::read(temporary.path().join("release.json"))?;
        let mut invalid = original.clone();
        invalid.release.as_mut().unwrap().tag = "v9.9.9".into();
        assert!(store.write(&invalid).is_err());
        assert_eq!(fs::read(temporary.path().join("release.json"))?, bytes);
        let mut later = original;
        later.phase = Phase::Complete;
        later.error = Some("diagnostic evidence, not completion authority".into());
        store.write(&later)?;
        assert_eq!(store.read()?, Some(later));
        fs::write(temporary.path().join("release.json"), "not JSON")?;
        assert!(store.read().is_err());
        Ok(())
    }

    #[test]
    fn invalid_or_forged_archive_records_fail_before_file_discovery() -> Result {
        let mut receipt = receipt()?;
        receipt.artifact = Some(ArtifactRecord {
            filename: "../outside.crate".into(),
            checksum: "b".repeat(64),
            size: 12,
        });
        assert!(receipt.validate().is_err());
        receipt.artifact.as_mut().unwrap().filename = "fixture-0.1.0.crate".into();
        receipt.verification = Verification::Verified {
            checksum: "c".repeat(64),
        };
        assert!(receipt.validate().is_err());
        receipt.verification = Verification::Pending;
        receipt.next_version = Some(Version::new(0, 1, 0));
        assert!(receipt.validate().is_err());
        let initialization_error = ReleaseReceipt {
            error: Some("initialization failed before capturing a release".into()),
            ..ReleaseReceipt::default()
        };
        initialization_error.validate()?;
        Ok(())
    }
}
