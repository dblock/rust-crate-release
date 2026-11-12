//! Data-only configuration and the built-in release commands.

use crate::changelog::{
    ChangelogTemplate, GeneratedChangelog, IntrideaChangelog, KeepAChangelog, NoChangelog,
};
use crate::engine::{
    Analyzed, Captured, CapturedInputs, CommandValidator, Edited, Hosted, NextEdited, NextVersion,
    RegistryVerifier, ReleaseRequest, ReleaseWorkflow, SystemClock,
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

#[derive(Clone, Debug, Parser)]
#[command(name = "rcr", version, about = "Release an exact Rust crate archive")]
pub struct Cli {
    #[arg(long, global = true, default_value = ".")]
    pub root: PathBuf,
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    #[arg(long, global = true)]
    pub artifact_directory: Option<PathBuf>,
    /// Immutable GitHub upload to verify independently before publication.
    #[arg(long, global = true, hide = true)]
    pub github_artifact_id: Option<u64>,
    /// Explicit loopback artifact API; requires the isolated publication adapter.
    #[arg(long, global = true, hide = true, requires = "test_registry")]
    pub test_artifact_api: Option<String>,
    /// Explicit isolated publication adapter; never auto-selected.
    #[arg(long, global = true, hide = true)]
    pub test_registry: Option<PathBuf>,
    /// Explicit isolated hosting adapter; requires isolated publication.
    #[arg(long, global = true, hide = true, requires = "test_registry")]
    pub test_host: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// GitHub Actions preparation and publication using the same Rust engine.
    Actions {
        #[command(subcommand)]
        command: crate::actions::ActionCommand,
    },
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
    Disabled,
    KeepAChangelog,
    Intridea,
    Generated,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Configuration {
    pub prepare_command: Option<Vec<String>>,
    pub publish_command: Option<Vec<String>>,
    pub bump_command: Option<Vec<String>>,
    pub success: Option<Vec<String>>,
    pub failure: Option<Vec<String>>,
    pub runner: Option<Vec<String>>,
    pub workflow: Mode,
    pub manifest: PathBuf,
    /// Empty selects Actions repository metadata or the remote's advertised HEAD.
    pub default_branch: String,
    pub remote: String,
    pub timezone: String,
    pub changelog: PathBuf,
    pub changelog_format: Option<ChangelogFormat>,
    pub placeholder: Option<String>,
    pub sections: Vec<String>,
    pub next_version: Option<String>,
    pub validation: Vec<Vec<String>>,
    /// Enables hosting when a GitHub repository is configured or discovered.
    pub github_release: bool,
    pub github_repository: Option<String>,
    pub artifact_directory: Option<PathBuf>,
    pub verification_attempts: usize,
    pub verification_delay_seconds: u64,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            prepare_command: None,
            publish_command: None,
            bump_command: None,
            success: None,
            failure: None,
            runner: None,
            workflow: Mode::Numeric,
            manifest: "Cargo.toml".into(),
            default_branch: String::new(),
            remote: "origin".into(),
            timezone: "UTC".into(),
            changelog: "CHANGELOG.md".into(),
            changelog_format: None,
            placeholder: None,
            sections: Vec::new(),
            next_version: None,
            validation: CommandValidator::default().commands,
            github_release: true,
            github_repository: None,
            artifact_directory: None,
            verification_attempts: 6,
            verification_delay_seconds: 5,
        }
    }
}

impl Configuration {
    fn branch(&self, root: &Path, values: &impl Fn(&str) -> Option<String>) -> Result<String> {
        if !self.default_branch.is_empty() {
            return Ok(self.default_branch.clone());
        }
        if values("GITHUB_ACTIONS").as_deref() == Some("true") {
            if let Some(branch) = values("RCR_DEFAULT_BRANCH").filter(|value| !value.is_empty()) {
                return Ok(branch);
            }
            if let Some(path) = values("GITHUB_EVENT_PATH").filter(|value| !value.is_empty()) {
                let event: Value = serde_json::from_slice(&fs::read(path)?)?;
                if let Some(branch) = event["repository"]["default_branch"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                {
                    return Ok(branch.into());
                }
            }
        }
        Git::default_branch(root, &self.remote)
    }

    fn hosting_repository(
        &self,
        root: &Path,
        values: &impl Fn(&str) -> Option<String>,
    ) -> Result<Option<String>> {
        if !self.github_release {
            return Ok(None);
        }
        if let Some(repository) = &self.github_repository {
            return Ok(Some(repository.clone()));
        }
        if values("GITHUB_ACTIONS").as_deref() == Some("true")
            && values("GITHUB_SERVER_URL").is_none_or(|value| value == "https://github.com")
            && let Some(repository) = values("GITHUB_REPOSITORY").filter(|value| !value.is_empty())
        {
            crate::hosting::validate_repository(&repository)?;
            return Ok(Some(repository));
        }
        let remote = crate::command::text(crate::command::run(
            root,
            "git",
            &["remote", "get-url", &self.remote],
        )?)?;
        repository_from_remote(&remote)
    }

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
        if config
            .runner
            .as_ref()
            .is_some_and(|argv| argv.first().is_none_or(String::is_empty))
        {
            return Err(Error::invalid("runner requires nonempty program argv"));
        }
        for argv in [
            &config.success,
            &config.failure,
            &config.prepare_command,
            &config.publish_command,
            &config.bump_command,
        ]
        .into_iter()
        .flatten()
        {
            if argv.first().is_none_or(String::is_empty) {
                return Err(Error::invalid("callbacks require nonempty program argv"));
            }
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
        if matches!(config.changelog_format, Some(ChangelogFormat::Disabled))
            && (config.placeholder.is_some() || !config.sections.is_empty())
        {
            return Err(Error::invalid(
                "disabled changelogs do not accept handwritten templates",
            ));
        }
        config.verifier()?;
        SystemClock::new(&config.timezone)?;
        config.next()?;
        ChangelogTemplate::new(config.placeholder.clone(), config.sections.clone())?;
        Ok(config)
    }
}

fn repository_from_remote(remote: &str) -> Result<Option<String>> {
    let path = if let Some(path) = remote.strip_prefix("git@github.com:") {
        path.to_owned()
    } else {
        let Ok(url) = reqwest::Url::parse(remote) else {
            return Ok(None);
        };
        if url.host_str() != Some("github.com")
            || !matches!(url.scheme(), "https" | "ssh" | "git")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Ok(None);
        }
        url.path().trim_start_matches('/').to_owned()
    };
    let path = path.trim_end_matches('/');
    let repository = path.strip_suffix(".git").unwrap_or(path);
    crate::hosting::validate_repository(repository)?;
    Ok(Some(repository.into()))
}

fn phase_command(key: &str, fallback: Option<Vec<String>>) -> Result<Option<Vec<String>>> {
    let command = std::env::var(key)
        .ok()
        .filter(|value| !value.is_empty())
        .map(|value| {
            serde_json::from_str::<Vec<String>>(&value)
                .map_err(|_| Error::invalid(format!("{key} must be JSON argv")))
        })
        .transpose()?
        .or(fallback);
    if command
        .as_ref()
        .is_some_and(|argv| argv.first().is_none_or(String::is_empty))
    {
        return Err(Error::invalid(format!(
            "{key} requires nonempty program argv"
        )));
    }
    Ok(command)
}

fn execute_phase(root: &Path, argv: &[String], environment: &[(&str, &str)]) -> Result {
    let args: Vec<_> = argv[1..].iter().map(String::as_str).collect();
    let output = crate::command::run_with_env_and_remove(
        root,
        &argv[0],
        &args,
        environment,
        &["CRATE_ARCHIVE"],
    )?;
    std::io::stderr().write_all(&output.stdout)?;
    std::io::stderr().write_all(&output.stderr)?;
    Ok(())
}

struct EditCommand {
    root: PathBuf,
    argv: Vec<String>,
}

impl LifecycleHook<Analyzed, Edited> for EditCommand {
    fn id(&self) -> &str {
        "prepare-command"
    }
    fn after(&self, output: &Edited, _: &ExecutionContext) -> Result {
        execute_phase(
            &self.root,
            &self.argv,
            &[(
                "RCR_RELEASE_VERSION",
                &output.release().version().to_string(),
            )],
        )
    }
}

impl LifecycleHook<Hosted, NextEdited> for EditCommand {
    fn id(&self) -> &str {
        "bump-command"
    }
    fn after(&self, output: &NextEdited, _: &ExecutionContext) -> Result {
        if let Some(next) = output.next_version() {
            execute_phase(
                &self.root,
                &self.argv,
                &[
                    (
                        "RCR_RELEASE_VERSION",
                        &output.release().version().to_string(),
                    ),
                    ("RCR_NEXT_VERSION", &next.to_string()),
                ],
            )?;
        }
        Ok(())
    }
}

struct CommandPublisher {
    root: PathBuf,
    argv: Vec<String>,
    inner: Arc<dyn PackagePublisher>,
}

impl PackagePublisher for CommandPublisher {
    fn lookup(
        &self,
        name: &str,
        version: &Version,
    ) -> Result<Option<crate::registry::PublishedCrate>> {
        self.inner.lookup(name, version)
    }
    fn download(&self, name: &str, version: &Version) -> Result<Vec<u8>> {
        self.inner.download(name, version)
    }
    fn publish(
        &self,
        artifact: &Artifact,
        _: &dyn crate::execution::ExecutionProvider,
    ) -> Result<crate::registry::PublishOutcome> {
        artifact.verify()?;
        let existing = self.lookup(artifact.name(), artifact.version())?;
        if existing.is_none() {
            execute_phase(
                &self.root,
                &self.argv,
                &[
                    (
                        "CRATE_ARCHIVE",
                        artifact
                            .path()
                            .to_str()
                            .ok_or_else(|| Error::invalid("archive path must be UTF-8"))?,
                    ),
                    ("RCR_RELEASE_VERSION", &artifact.version().to_string()),
                ],
            )?;
        }
        let published = self.lookup(artifact.name(), artifact.version())?
                .ok_or_else(|| Error::invalid("publish command did not expose the released version; reconcile before retrying"))?;
        if published.checksum != artifact.checksum()
            || self.download(artifact.name(), artifact.version())? != artifact.validated_bytes()?
        {
            return Err(Error::invalid(
                "publish command did not publish the exact retained archive",
            ));
        }
        Ok(if existing.is_some() {
            crate::registry::PublishOutcome::Reused
        } else {
            crate::registry::PublishOutcome::Published
        })
    }
}

impl Configuration {
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
    if matches!(
        cli.command,
        Command::Actions {
            command: crate::actions::ActionCommand::Notify
        }
    ) {
        return run_with(cli, &|_| Ok(()));
    }
    let root = dunce::canonicalize(&cli.root)?;
    let config_path = cli.config.clone().or_else(|| {
        std::env::var_os("RCR_CONFIG")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    });
    let config = Configuration::load(&root, config_path.as_deref())?;
    let runner = std::env::var("RCR_RUNNER")
        .ok()
        .filter(|value| !value.is_empty())
        .map(|value| {
            serde_json::from_str::<Vec<String>>(&value)
                .map_err(|_| Error::invalid("RCR_RUNNER must be JSON argv"))
        })
        .transpose()?
        .or(config.runner);
    if let Some(argv) = runner {
        if std::env::var_os("RCR_RUNNER_ACTIVE").is_some() {
            return Err(Error::invalid(
                "project runners must call cli::main_with or cli::run_with",
            ));
        }
        let (program, prefix) = argv
            .split_first()
            .filter(|(program, _)| !program.is_empty())
            .ok_or_else(|| Error::invalid("runner requires nonempty program argv"))?;
        let output = std::process::Command::new(program)
            .args(prefix)
            .args(forward_arguments(&cli, &root))
            .current_dir(&root)
            .env("RCR_RUNNER_ACTIVE", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()?;
        std::io::stderr().write_all(&output.stderr)?;
        if !output.status.success() {
            return Err(Error::invalid(format!(
                "project runner failed ({})",
                output.status
            )));
        }
        return serde_json::from_slice(&output.stdout)
            .map_err(|_| Error::invalid("project runner must return one JSON value"));
    }
    run_with(cli, &|_| Ok(()))
}

fn forward_arguments(cli: &Cli, root: &Path) -> Vec<std::ffi::OsString> {
    let mut args = vec!["--root".into(), root.as_os_str().to_owned()];
    let mut option = |flag: &str, value: Option<&Path>| {
        if let Some(value) = value {
            args.extend([flag.into(), value.as_os_str().to_owned()]);
        }
    };
    option("--config", cli.config.as_deref());
    option("--artifact-directory", cli.artifact_directory.as_deref());
    option("--test-registry", cli.test_registry.as_deref());
    option("--test-host", cli.test_host.as_deref());
    if let Some(id) = cli.github_artifact_id {
        args.extend(["--github-artifact-id".into(), id.to_string().into()]);
    }
    if let Some(endpoint) = &cli.test_artifact_api {
        args.extend(["--test-artifact-api".into(), endpoint.into()]);
    }
    match &cli.command {
        Command::Actions { command } => args.extend([
            "actions".into(),
            match command {
                crate::actions::ActionCommand::Guard => "guard",
                crate::actions::ActionCommand::Prepare => "prepare",
                crate::actions::ActionCommand::Release => "release",
                crate::actions::ActionCommand::Notify => "notify",
            }
            .into(),
        ]),
        Command::Prepare(release) | Command::Release(release) => {
            args.push(
                if matches!(cli.command, Command::Prepare(_)) {
                    "prepare"
                } else {
                    "release"
                }
                .into(),
            );
            if let Some(version) = &release.release_version {
                args.extend(["--release-version".into(), version.to_string().into()]);
            }
            for (flag, value) in [
                ("--archive", &release.archive),
                ("--inputs", &release.inputs),
                ("--save-inputs", &release.save_inputs),
            ] {
                if let Some(path) = value {
                    args.extend([flag.into(), path.as_os_str().to_owned()]);
                }
            }
        }
        Command::Verify {
            name,
            release_version,
            archive,
        } => args.extend([
            "verify".into(),
            "--name".into(),
            name.into(),
            "--release-version".into(),
            release_version.to_string().into(),
            "--archive".into(),
            archive.as_os_str().to_owned(),
        ]),
    }
    args
}

/// Project runner entry point retaining stock parsing, output, and safety checks.
pub fn main_with(configure: impl Fn(&mut ReleaseWorkflow) -> Result) -> std::process::ExitCode {
    match run_with(Cli::parse(), &configure) {
        Ok(output) => {
            println!("{output}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("rcr: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Configures trusted project providers/hooks without replacing CLI orchestration.
pub fn run_with(cli: Cli, configure: &dyn Fn(&mut ReleaseWorkflow) -> Result) -> Result<Value> {
    if let Command::Actions { command } = &cli.command {
        return crate::actions::run(&cli, *command, configure);
    }
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
        Command::Verify { .. } | Command::Actions { .. } => unreachable!(),
    };
    let values = |key: &str| std::env::var(key).ok();
    let branch = config.branch(&root, &values)?;
    let repository = Arc::new(Git::new(&root, &branch, &config.remote)?);
    let hosting_repository = if cli.test_registry.is_none() {
        config.hosting_repository(&root, &values)?
    } else {
        None
    };
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
        Some(ChangelogFormat::Disabled) => Arc::new(NoChangelog),
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
        commands: {
            let mut commands = phase_command("RCR_VALIDATE_COMMAND", None)?
                .map(|command| vec![command])
                .unwrap_or(config.validation);
            if let Some(command) = phase_command("RCR_TEST_COMMAND", None)? {
                commands.push(command);
            }
            commands
        },
    });
    workflow.components.next_version = Arc::new(next);
    workflow.components.verifier = Arc::new(verifier);
    workflow.components.execution = Arc::from(execution_provider(&branch));
    if let Some(id) = cli.github_artifact_id {
        workflow.components.preserver = Arc::new(match cli.test_artifact_api {
            Some(endpoint) => crate::actions::GitHubArtifacts::for_test_server(id, &endpoint)?,
            None => crate::actions::GitHubArtifacts::new(id)?,
        });
    }
    if let Some(repository) = hosting_repository {
        workflow.components.host = Some(Arc::new(GitHubReleaseHost::new(&repository)?));
    }
    if let Some(path) = cli.test_host {
        workflow.components.host = Some(Arc::new(crate::testing::DirectoryReleaseHost::new(
            root.join(path),
        )?));
    }
    if let Some(argv) = phase_command("RCR_PREPARE_COMMAND", config.prepare_command)? {
        workflow.hooks.prepare = workflow.hooks.prepare.with(EditCommand {
            root: root.clone(),
            argv,
        });
    }
    if let Some(argv) = phase_command("RCR_BUMP_COMMAND", config.bump_command)? {
        workflow.hooks.next = workflow.hooks.next.with(EditCommand {
            root: root.clone(),
            argv,
        });
    }
    let configured_publisher = if cli.test_registry.is_some() {
        None
    } else {
        config.publish_command
    };
    if let Some(argv) = phase_command("RCR_PUBLISH_COMMAND", configured_publisher)? {
        workflow.components.publisher = Arc::new(CommandPublisher {
            root: root.clone(),
            argv,
            inner: workflow.components.publisher.clone(),
        });
    }
    if operation == "release" && std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        workflow
            .observers
            .push(Arc::new(crate::execution::CommandCallbacks {
                root: root.clone(),
                success: config.success,
                failure: config.failure,
            }));
    }
    configure(&mut workflow)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::GitProject;
    use std::collections::BTreeMap;

    #[test]
    fn repository_defaults_use_actions_metadata_and_respect_explicit_overrides() -> Result {
        let project = GitProject::cargo_library()?;
        let root = project.root();
        let mut config = Configuration::load(root, None)?;
        assert_eq!(config.branch(root, &|_| None)?, "master");
        assert!(Configuration::load(root, Some(Path::new("missing.toml"))).is_err());
        let mut values: BTreeMap<String, String> = [
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_SERVER_URL", "https://github.com"),
            ("GITHUB_REPOSITORY", "consumer/widget"),
            ("RCR_DEFAULT_BRANCH", "main"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
        assert_eq!(
            config.branch(root, &|key| values.get(key).cloned())?,
            "main"
        );
        assert_eq!(
            config.hosting_repository(root, &|key| values.get(key).cloned())?,
            Some("consumer/widget".into())
        );
        values.remove("RCR_DEFAULT_BRANCH");
        let event = project.temporary_directory().join("event.json");
        fs::write(&event, r#"{"repository":{"default_branch":"trunk"}}"#)?;
        values.insert("GITHUB_EVENT_PATH".into(), event.to_string_lossy().into());
        assert_eq!(
            config.branch(root, &|key| values.get(key).cloned())?,
            "trunk"
        );
        config.default_branch = "release".into();
        config.github_repository = Some("override/project".into());
        assert_eq!(
            config.branch(root, &|key| values.get(key).cloned())?,
            "release"
        );
        assert_eq!(
            config.hosting_repository(root, &|key| values.get(key).cloned())?,
            Some("override/project".into())
        );
        config.github_release = false;
        assert_eq!(config.hosting_repository(root, &|_| None)?, None);
        Ok(())
    }

    #[test]
    fn local_repository_inference_uses_the_configured_remote_without_actions_environment() -> Result
    {
        let project = GitProject::cargo_library()?;
        crate::command::run(
            project.root(),
            "git",
            &["remote", "rename", "origin", "upstream"],
        )?;
        fs::write(project.root().join("rcr.toml"), "remote = \"upstream\"\n")?;
        let config = Configuration::load(project.root(), None)?;
        let values = |key: &str| match key {
            "RCR_DEFAULT_BRANCH" => Some("not-the-default".into()),
            "GITHUB_REPOSITORY" => Some("not-the/project".into()),
            _ => None,
        };
        assert_eq!(config.branch(project.root(), &values)?, "master");
        assert_eq!(config.hosting_repository(project.root(), &values)?, None);
        crate::command::run(
            project.root(),
            "git",
            &[
                "remote",
                "set-url",
                "upstream",
                "git@github.com:acme/widget.git",
            ],
        )?;
        assert_eq!(
            config.hosting_repository(project.root(), &values)?,
            Some("acme/widget".into())
        );
        let enterprise = |key: &str| match key {
            "GITHUB_ACTIONS" => Some("true".into()),
            "GITHUB_SERVER_URL" => Some("https://github.example.com".into()),
            "GITHUB_REPOSITORY" => Some("not-the/project".into()),
            _ => None,
        };
        assert_eq!(
            config.hosting_repository(project.root(), &enterprise)?,
            Some("acme/widget".into())
        );
        Ok(())
    }

    #[test]
    fn hosting_inference_accepts_only_literal_github_repository_urls() -> Result {
        for remote in [
            "https://github.com/acme/widget",
            "https://github.com/acme/widget.git",
            "git@github.com:acme/widget.git",
            "ssh://git@github.com/acme/widget.git",
            "git://github.com/acme/widget.git",
        ] {
            assert_eq!(repository_from_remote(remote)?, Some("acme/widget".into()));
        }
        for remote in [
            "/tmp/remote.git",
            "https://gitlab.com/acme/widget.git",
            "git@github.example.com:acme/widget.git",
            "https://github.com.example.com/acme/widget.git",
            "https://github.com/acme/widget?query",
            "https://github.com/acme/widget#fragment",
        ] {
            assert_eq!(repository_from_remote(remote)?, None);
        }
        for remote in [
            "https://github.com/acme",
            "https://github.com/acme/widget/extra",
            "git@github.com:acme/widget%2fother.git",
        ] {
            assert!(repository_from_remote(remote).is_err());
        }
        Ok(())
    }
}
