//! Data-only configuration and the built-in release commands.

use crate::changelog::{ChangelogTemplate, GeneratedChangelog, IntrideaChangelog, KeepAChangelog};
use crate::engine::{
    Analyzed, Captured, CapturedInputs, CommandValidator, NextVersion, RegistryVerifier,
    ReleaseRequest, ReleaseWorkflow, SystemClock,
};
use crate::execution::{ExecutionContext, execution_provider};
use crate::git::{Git, GitHistory, ReleaseRepository};
use crate::hooks::LifecycleHook;
use crate::hosting::GitHubReleaseHost;
use crate::package::{Artifact, CargoPackage};
use crate::registry::{CratesIoPublisher, PackagePublisher};
use crate::store::DirectoryReleaseStore;
use crate::testing::LocalRegistry;
use crate::{Error, Result};
use clap::{Args, Parser, Subcommand};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(name = "rcr", version, about = "Release an exact Rust crate archive")]
pub struct Cli {
    #[arg(long, global = true, default_value = ".")]
    pub root: PathBuf,
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    #[arg(long, global = true)]
    pub artifact_directory: Option<PathBuf>,
    /// Trusted helper from the reviewed composite action, not data-only configuration.
    #[arg(long, global = true, hide = true)]
    pub github_artifact_uploader: Option<PathBuf>,
    /// Explicit isolated publication adapter; never auto-selected.
    #[arg(long, global = true, hide = true)]
    pub test_registry: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Validate, commit, package, tag, and retain; do not publish or push.
    Prepare(ReleaseArgs),
    /// Reconcile and complete the release, then verify the released archive.
    Release(ReleaseArgs),
    /// Read-only verification of an explicit released identity and archive.
    Verify {
        #[arg(long)]
        name: String,
        #[arg(long)]
        release_version: Version,
        #[arg(long)]
        archive: PathBuf,
    },
}

#[derive(Clone, Debug, Args)]
pub struct ReleaseArgs {
    #[arg(long)]
    pub release_version: Option<Version>,
    #[arg(long, value_parser = parse_archive)]
    pub archive: Option<PathBuf>,
    /// Explicit immutable source inputs, not a workflow checkpoint.
    #[arg(long)]
    pub inputs: Option<PathBuf>,
    /// Save immutable source inputs before preparation edits.
    #[arg(long)]
    pub save_inputs: Option<PathBuf>,
}

fn parse_archive(value: &str) -> std::result::Result<PathBuf, std::convert::Infallible> {
    Ok(PathBuf::from(value))
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    #[default]
    Numeric,
    Semantic,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangelogFormat {
    KeepAChangelog,
    Intridea,
    Generated,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Configuration {
    pub workflow: Mode,
    pub manifest: PathBuf,
    pub default_branch: String,
    pub remote: String,
    pub timezone: String,
    pub changelog: PathBuf,
    pub changelog_format: Option<ChangelogFormat>,
    pub placeholder: Option<String>,
    pub sections: Vec<String>,
    pub next_version: Option<String>,
    pub validation: Vec<Vec<String>>,
    pub github_repository: Option<String>,
    pub artifact_directory: Option<PathBuf>,
    pub verification_attempts: usize,
    pub verification_delay_seconds: u64,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            workflow: Mode::Numeric,
            manifest: "Cargo.toml".into(),
            default_branch: "master".into(),
            remote: "origin".into(),
            timezone: "UTC".into(),
            changelog: "CHANGELOG.md".into(),
            changelog_format: None,
            placeholder: None,
            sections: Vec::new(),
            next_version: None,
            validation: CommandValidator::default().commands,
            github_repository: None,
            artifact_directory: None,
            verification_attempts: 6,
            verification_delay_seconds: 5,
        }
    }
}

impl Configuration {
    pub fn load(root: &Path, explicit: Option<&Path>) -> Result<Self> {
        let path = root.join(explicit.unwrap_or_else(|| Path::new("rcr.toml")));
        let config: Self = match fs::read_to_string(path) {
            Ok(source) => toml::from_str(&source).map_err(|error: toml::de::Error| {
                Error::invalid(format!("invalid rcr configuration: {}", error.message()))
            })?,
            Err(error) if explicit.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
                Self::default()
            }
            Err(error) => return Err(error.into()),
        };
        if config.validation.is_empty()
            || config
                .validation
                .iter()
                .any(|args| args.first().is_none_or(String::is_empty))
        {
            return Err(Error::invalid(
                "configuration requires nonempty validation argv",
            ));
        }
        match (config.workflow, config.changelog_format) {
            (Mode::Numeric, Some(ChangelogFormat::Generated))
            | (Mode::Semantic, Some(ChangelogFormat::Intridea | ChangelogFormat::KeepAChangelog)) =>
            {
                return Err(Error::invalid(
                    "changelog format is incompatible with workflow",
                ));
            }
            _ => {}
        }
        if matches!(config.workflow, Mode::Semantic)
            && (config.placeholder.is_some() || !config.sections.is_empty())
        {
            return Err(Error::invalid(
                "generated changelogs do not accept handwritten templates",
            ));
        }
        config.verifier()?;
        SystemClock::new(&config.timezone)?;
        config.next()?;
        ChangelogTemplate::new(config.placeholder.clone(), config.sections.clone())?;
        Ok(config)
    }

    fn verifier(&self) -> Result<RegistryVerifier> {
        RegistryVerifier::new(
            self.verification_attempts,
            Duration::from_secs(self.verification_delay_seconds),
        )
    }

    fn next(&self) -> Result<NextVersion> {
        Ok(match self.next_version.as_deref() {
            Some("patch") => NextVersion::Patch,
            Some("disabled") => NextVersion::Disabled,
            Some(version) => {
                NextVersion::Exact(Version::parse(version).map_err(|_| {
                    Error::invalid("next-version must be patch, disabled, or SemVer")
                })?)
            }
            None if matches!(self.workflow, Mode::Semantic) => NextVersion::Disabled,
            None => NextVersion::Patch,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    repository: String,
    run: String,
    attempt: String,
    workflow: String,
}

fn binding() -> Result<Option<Binding>> {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return Ok(None);
    }
    let value = |key| {
        std::env::var(key)
            .ok()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                Error::invalid("GitHub source-input transport requires complete run identity")
            })
    };
    Ok(Some(Binding {
        repository: value("GITHUB_REPOSITORY")?,
        run: value("GITHUB_RUN_ID")?,
        attempt: value("GITHUB_RUN_ATTEMPT")?,
        workflow: value("GITHUB_WORKFLOW_REF")?,
    }))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputFile {
    inputs: CapturedInputs,
    binding: Option<Binding>,
}

fn load_inputs(path: &Path) -> Result<CapturedInputs> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(Error::invalid("source inputs must be a regular file"));
    }
    let file: InputFile = serde_json::from_slice(&fs::read(path)?)?;
    if let Some(current) = binding()?
        && file.binding.as_ref() != Some(&current)
    {
        return Err(Error::invalid(
            "source inputs belong to another GitHub run or attempt",
        ));
    }
    Ok(file.inputs)
}

struct SaveInputs(PathBuf);

impl LifecycleHook<Captured, Analyzed> for SaveInputs {
    fn id(&self) -> &str {
        "save-source-inputs"
    }

    fn after(&self, output: &Analyzed, _: &ExecutionContext) -> Result {
        let bytes = serde_json::to_vec_pretty(&InputFile {
            inputs: output.captured().clone(),
            binding: binding()?,
        })?;
        let parent = self
            .0
            .parent()
            .ok_or_else(|| Error::invalid("input file needs a parent"))?;
        fs::create_dir_all(parent)?;
        if self.0.exists() || fs::symlink_metadata(&self.0).is_ok() {
            if fs::symlink_metadata(&self.0)?.is_file() && fs::read(&self.0)? == bytes {
                return Ok(());
            }
            return Err(Error::invalid(
                "existing source inputs differ; refusing to overwrite",
            ));
        }
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary
            .persist_noclobber(&self.0)
            .map_err(|error| error.error)?;
        Ok(())
    }
}

fn publisher(
    root: &Path,
    test: Option<&Path>,
    readonly: bool,
) -> Result<Arc<dyn PackagePublisher>> {
    if let Some(path) = test {
        let path = root.join(path);
        if readonly && !path.is_dir() {
            return Err(Error::invalid(
                "verification requires an existing test registry",
            ));
        }
        return Ok(Arc::new(LocalRegistry::new(path)?));
    }
    Ok(Arc::new(CratesIoPublisher::new()?))
}

fn archive_option(root: &Path, explicit: Option<PathBuf>) -> Option<PathBuf> {
    explicit
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| {
            std::env::var_os("CRATE_ARCHIVE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .map(|path| root.join(path))
}

/// Runs a built-in command. Successful output is one JSON value; progress uses stderr.
pub fn run(cli: Cli) -> Result<Value> {
    let root = dunce::canonicalize(&cli.root)?;
    let config = Configuration::load(&root, cli.config.as_deref())?;
    if let Command::Verify {
        name,
        release_version,
        archive,
    } = &cli.command
    {
        let artifact = Artifact::open(&root.join(archive))?;
        let publisher = publisher(&root, cli.test_registry.as_deref(), true)?;
        let verification = config.verifier()?.verify_archive(
            name,
            release_version,
            &artifact,
            publisher.as_ref(),
        )?;
        return Ok(
            json!({"operation":"verify","name":name,"version":release_version,"verification":verification}),
        );
    }
    let (operation, args) = match cli.command {
        Command::Prepare(args) => ("prepare", args),
        Command::Release(args) => ("release", args),
        Command::Verify { .. } => unreachable!(),
    };
    let repository = Arc::new(Git::new(&root, &config.default_branch, &config.remote)?);
    let package = Arc::new(CargoPackage::from_manifest(root.join(&config.manifest))?);
    let directory = cli
        .artifact_directory
        .as_ref()
        .or(config.artifact_directory.as_ref())
        .map(|path| root.join(path))
        .map(Ok)
        .unwrap_or_else(|| repository.artifact_directory())?;
    let mut workflow = ReleaseWorkflow::new(
        repository.clone(),
        package,
        publisher(&root, cli.test_registry.as_deref(), false)?,
        Arc::new(DirectoryReleaseStore::new(directory)?),
    );
    if matches!(config.workflow, Mode::Semantic) {
        workflow = workflow.semantic(Arc::new(GitHistory::new((*repository).clone())));
    }
    workflow.components.changelog = match config.changelog_format {
        Some(ChangelogFormat::Intridea) => {
            if !config.sections.is_empty() {
                return Err(Error::invalid("Intridea sections are not configurable"));
            }
            Arc::new(IntrideaChangelog::new(
                config
                    .placeholder
                    .clone()
                    .unwrap_or_else(|| "- Your contribution here.".into()),
            )?)
        }
        Some(ChangelogFormat::Generated) => Arc::new(GeneratedChangelog),
        _ if matches!(config.workflow, Mode::Semantic) => Arc::new(GeneratedChangelog),
        _ => Arc::new(KeepAChangelog::new(ChangelogTemplate::new(
            config.placeholder.clone(),
            config.sections.clone(),
        )?)),
    };
    let next = config.next()?;
    let verifier = config.verifier()?;
    workflow.components.changelog_path = config.changelog;
    workflow.components.clock = Arc::new(SystemClock::new(&config.timezone)?);
    workflow.components.validator = Arc::new(CommandValidator {
        commands: config.validation,
    });
    workflow.components.next_version = Arc::new(next);
    workflow.components.verifier = Arc::new(verifier);
    workflow.components.execution = Arc::from(execution_provider(&config.default_branch));
    if let Some(helper) = cli.github_artifact_uploader {
        workflow.components.preserver =
            Arc::new(crate::actions::GitHubArtifacts::new(root.join(helper))?);
    }
    if let Some(repository) = config.github_repository {
        workflow.components.host = Some(Arc::new(GitHubReleaseHost::new(&repository)?));
    }
    if let Some(path) = args.save_inputs {
        workflow.hooks.analyze = workflow.hooks.analyze.with(SaveInputs(root.join(path)));
    }
    let captured = args
        .inputs
        .map(|path| load_inputs(&root.join(path)))
        .transpose()?;
    let version = args.release_version.or_else(|| {
        captured
            .as_ref()
            .and_then(|inputs| inputs.version_override.clone())
    });
    let request = ReleaseRequest {
        version,
        captured,
        archive: archive_option(&root, args.archive),
    };
    macro_rules! result {
        ($output:expr) => {{
            let output = $output?;
            let archive = output.artifact().ok_or_else(|| Error::invalid("release produced no archive"))?;
            Ok(json!({
                "operation":operation,"name":output.release().name(),"version":output.release().version(),
                "commit":output.commit(),"archive":archive.path(),"checksum":archive.checksum(),
                "captured":output.captured(),"next_version":output.next_version(),
                "development_commit":output.development_commit(),"verification":output.verification(),
            }))
        }}
    }
    if operation == "prepare" {
        result!(workflow.prepare(request))
    } else {
        result!(workflow.release(request))
    }
}
