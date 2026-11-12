#![doc = include_str!("../docs/releases.md")]

use crate::changelog::{Changelog, GeneratedChangelog, KeepAChangelog};
use crate::command;
use crate::execution::{ExecutionContext, ExecutionProvider, LocalExecutionProvider, StepOutcome};
use crate::git::{ReleaseRepository, RepositoryState};
use crate::hooks::LifecycleHooks;
use crate::hosting::ReleaseHost;
use crate::package::{Artifact, PackageProject};
use crate::registry::PackagePublisher;
use crate::semantic::{SemanticDecision, SemanticHistory, SemanticRelease};
use crate::store::{Phase, ReleaseReceipt, ReleaseStore, Verification};
use crate::{Error, Release, Result, RunOptions, Step, Workflow, WorkflowObserver};
use chrono::Utc;
use chrono_tz::Tz;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub trait ReleaseClock: Send + Sync {
    fn date(&self) -> Result<String>;
}

#[derive(Clone, Debug)]
pub struct SystemClock(Tz);

impl SystemClock {
    pub fn new(timezone: &str) -> Result<Self> {
        Ok(Self(timezone.parse().map_err(|_| {
            Error::invalid("unknown IANA release timezone")
        })?))
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self(chrono_tz::UTC)
    }
}

impl ReleaseClock for SystemClock {
    fn date(&self) -> Result<String> {
        Ok(Utc::now()
            .with_timezone(&self.0)
            .format("%Y/%m/%d")
            .to_string())
    }
}

pub trait DevelopmentVersionPolicy: Send + Sync {
    fn next(&self, release: &Release) -> Result<Option<Version>>;
}

#[derive(Clone, Debug, Default)]
pub enum NextVersion {
    #[default]
    Patch,
    Disabled,
    Exact(Version),
}

impl DevelopmentVersionPolicy for NextVersion {
    fn next(&self, release: &Release) -> Result<Option<Version>> {
        let next = match self {
            Self::Disabled => return Ok(None),
            Self::Exact(version) => version.clone(),
            Self::Patch => Version::new(
                release.version().major,
                release.version().minor,
                release
                    .version()
                    .patch
                    .checked_add(1)
                    .ok_or_else(|| Error::invalid("next version overflows SemVer"))?,
            ),
        };
        if next <= *release.version() || !next.pre.is_empty() || !next.build.is_empty() {
            return Err(Error::invalid("next version must be stable and increasing"));
        }
        Ok(Some(next))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationStage {
    Source,
    Development,
}

pub struct ValidationInput<'a> {
    pub release: &'a Release,
    pub stage: ValidationStage,
    pub package: &'a dyn PackageProject,
    pub archive: Option<&'a Path>,
}

pub trait ReleaseValidator: Send + Sync {
    fn validate(&self, input: ValidationInput<'_>, execution: &ExecutionContext) -> Result;
}

#[derive(Clone, Debug)]
pub struct CommandValidator {
    pub commands: Vec<Vec<String>>,
}

impl Default for CommandValidator {
    fn default() -> Self {
        Self {
            commands: [
                vec!["cargo", "fmt", "--all", "--", "--check"],
                vec!["cargo", "test", "--locked"],
                vec![
                    "cargo",
                    "clippy",
                    "--locked",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
            ]
            .into_iter()
            .map(|command| command.into_iter().map(str::to_owned).collect())
            .collect(),
        }
    }
}

impl ReleaseValidator for CommandValidator {
    fn validate(&self, input: ValidationInput<'_>, _: &ExecutionContext) -> Result {
        if self.commands.is_empty() {
            return Err(Error::invalid(
                "at least one validation command is required",
            ));
        }
        let archive = input
            .archive
            .filter(|_| input.stage == ValidationStage::Source)
            .map(|path| {
                path.to_str()
                    .ok_or_else(|| Error::invalid("archive path must be UTF-8"))
            })
            .transpose()?;
        let environment: Vec<_> = archive
            .into_iter()
            .map(|path| ("CRATE_ARCHIVE", path))
            .collect();
        for command in &self.commands {
            let (program, args) = command
                .split_first()
                .filter(|(program, _)| !program.is_empty())
                .ok_or_else(|| Error::invalid("validation command must not be empty"))?;
            eprintln!("> {}", command.join(" "));
            let args: Vec<_> = args.iter().map(String::as_str).collect();
            let output = command::run_with_env_and_remove(
                input.package.root(),
                program,
                &args,
                &environment,
                &["CRATE_ARCHIVE"],
            )?;
            std::io::stderr().write_all(&output.stdout)?;
            std::io::stderr().write_all(&output.stderr)?;
        }
        Ok(())
    }
}

pub trait ArtifactPreserver: Send + Sync {
    fn preserve(
        &self,
        release: &Release,
        directory: &Path,
        execution: &dyn ExecutionProvider,
    ) -> Result;
}

#[derive(Clone, Debug, Default)]
pub struct LocalArtifacts;

impl ArtifactPreserver for LocalArtifacts {
    fn preserve(&self, _: &Release, _: &Path, execution: &dyn ExecutionProvider) -> Result {
        if execution.metadata().platform == "github" {
            return Err(Error::invalid(
                "GitHub publication requires a durable artifact uploader before registry mutation",
            ));
        }
        Ok(())
    }
}

pub trait ReleaseVerifier: Send + Sync {
    fn verify(
        &self,
        release: &Release,
        artifact: &Artifact,
        publisher: &dyn PackagePublisher,
    ) -> Result<Verification>;
}

#[derive(Clone, Debug)]
pub struct RegistryVerifier {
    attempts: usize,
    delay: Duration,
}

impl RegistryVerifier {
    pub fn new(attempts: usize, delay: Duration) -> Result<Self> {
        if !(1..=60).contains(&attempts) || delay > Duration::from_secs(60) {
            return Err(Error::invalid("verification retries must be bounded"));
        }
        Ok(Self { attempts, delay })
    }
}

impl Default for RegistryVerifier {
    fn default() -> Self {
        Self {
            attempts: 6,
            delay: Duration::from_secs(5),
        }
    }
}

impl ReleaseVerifier for RegistryVerifier {
    fn verify(
        &self,
        release: &Release,
        artifact: &Artifact,
        publisher: &dyn PackagePublisher,
    ) -> Result<Verification> {
        self.verify_archive(release.name(), release.version(), artifact, publisher)
    }
}

impl RegistryVerifier {
    /// Verifies an explicit released identity without inspecting a checkout.
    pub fn verify_archive(
        &self,
        name: &str,
        version: &Version,
        artifact: &Artifact,
        publisher: &dyn PackagePublisher,
    ) -> Result<Verification> {
        artifact.verify()?;
        if artifact.name() != name || artifact.version() != version {
            return Err(Error::invalid(
                "archive identity differs from explicit release",
            ));
        }
        for attempt in 1..=self.attempts {
            if let Some(published) = publisher.lookup(name, version)? {
                if published.name != name
                    || &published.version != version
                    || published.checksum != artifact.checksum()
                {
                    return Err(Error::invalid(
                        "published identity or checksum conflicts with release",
                    ));
                }
                if publisher.download(name, version)? != artifact.validated_bytes()? {
                    return Err(Error::invalid(
                        "published archive bytes differ from retained release",
                    ));
                }
                return Ok(Verification::Verified {
                    checksum: artifact.checksum().into(),
                });
            }
            if attempt < self.attempts {
                std::thread::sleep(self.delay);
            }
        }
        Ok(Verification::NotVisible {
            attempts: self.attempts,
        })
    }
}

/// Explicit captured inputs, not a workflow descriptor or completion journal.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturedInputs {
    pub name: String,
    pub manifest_version: Version,
    pub source_commit: String,
    pub remote_source: String,
    pub source_date_epoch: i64,
    pub date: String,
    pub version_override: Option<Version>,
    pub semantic: Option<SemanticDecision>,
}

#[derive(Clone, Debug, Default)]
pub struct ReleaseRequest {
    pub version: Option<Version>,
    pub captured: Option<CapturedInputs>,
    pub archive: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Captured {
    inputs: CapturedInputs,
    repository: RepositoryState,
    actual_version: Version,
    archive: Option<PathBuf>,
}

impl Captured {
    pub fn inputs(&self) -> &CapturedInputs {
        &self.inputs
    }
}

#[derive(Clone, Debug)]
struct RunData {
    capture: Captured,
    release: Release,
    notes: String,
    tree: Option<String>,
    commit: Option<String>,
    artifact: Option<Artifact>,
    next: Option<Version>,
    development_commit: Option<String>,
    verification: Verification,
    development: bool,
}

macro_rules! states {
    ($($name:ident),+ $(,)?) => {$(
        #[derive(Clone, Debug)]
        pub struct $name(RunData);
        impl $name {
            pub fn release(&self) -> &Release { &self.0.release }
            pub fn captured(&self) -> &CapturedInputs { &self.0.capture.inputs }
            pub fn notes(&self) -> &str { &self.0.notes }
            pub fn commit(&self) -> Option<&str> { self.0.commit.as_deref() }
            pub fn artifact(&self) -> Option<&Artifact> { self.0.artifact.as_ref() }
            pub fn verification(&self) -> &Verification { &self.0.verification }
            pub fn development_commit(&self) -> Option<&str> { self.0.development_commit.as_deref() }
            pub fn next_version(&self) -> Option<&Version> { self.0.next.as_ref() }
        }
    )+};
}

states!(
    Analyzed,
    Edited,
    Prepared,
    Validated,
    Committed,
    Packaged,
    Preserved,
    Published,
    Pushed,
    Hosted,
    NextEdited,
    Development,
    Completed
);

#[derive(Clone)]
pub struct ReleaseComponents {
    pub repository: Arc<dyn ReleaseRepository>,
    pub package: Arc<dyn PackageProject>,
    pub publisher: Arc<dyn PackagePublisher>,
    pub changelog: Arc<dyn Changelog>,
    pub changelog_path: PathBuf,
    pub store: Arc<dyn ReleaseStore>,
    pub execution: Arc<dyn ExecutionProvider>,
    pub clock: Arc<dyn ReleaseClock>,
    pub validator: Arc<dyn ReleaseValidator>,
    pub preserver: Arc<dyn ArtifactPreserver>,
    pub verifier: Arc<dyn ReleaseVerifier>,
    pub next_version: Arc<dyn DevelopmentVersionPolicy>,
    pub host: Option<Arc<dyn ReleaseHost>>,
    pub semantic: Option<Arc<SemanticRelease>>,
    pub history: Option<Arc<dyn SemanticHistory>>,
}

#[derive(Clone, Default)]
pub struct ReleaseHooks {
    pub capture: LifecycleHooks<ReleaseRequest, Captured>,
    pub analyze: LifecycleHooks<Captured, Analyzed>,
    pub prepare: LifecycleHooks<Analyzed, Edited>,
    pub validate: LifecycleHooks<Prepared, Validated>,
    pub commit: LifecycleHooks<Validated, Committed>,
    pub build: LifecycleHooks<Committed, Packaged>,
    pub preserve: LifecycleHooks<Packaged, Preserved>,
    pub publish: LifecycleHooks<Preserved, Published>,
    pub push: LifecycleHooks<Published, Pushed>,
    pub host: LifecycleHooks<Pushed, Hosted>,
    pub next: LifecycleHooks<Hosted, NextEdited>,
    pub verify: LifecycleHooks<Development, Completed>,
}

pub struct ReleaseWorkflow {
    pub components: ReleaseComponents,
    pub hooks: ReleaseHooks,
    pub observers: Vec<Arc<dyn WorkflowObserver>>,
}

impl ReleaseWorkflow {
    pub fn new(
        repository: Arc<dyn ReleaseRepository>,
        package: Arc<dyn PackageProject>,
        publisher: Arc<dyn PackagePublisher>,
        store: Arc<dyn ReleaseStore>,
    ) -> Self {
        Self {
            components: ReleaseComponents {
                repository,
                package,
                publisher,
                store,
                changelog: Arc::new(KeepAChangelog::default()),
                changelog_path: "CHANGELOG.md".into(),
                execution: Arc::new(LocalExecutionProvider),
                clock: Arc::new(SystemClock::default()),
                validator: Arc::new(CommandValidator::default()),
                preserver: Arc::new(LocalArtifacts),
                verifier: Arc::new(RegistryVerifier::default()),
                next_version: Arc::new(NextVersion::Patch),
                host: None,
                semantic: None,
                history: None,
            },
            hooks: ReleaseHooks::default(),
            observers: Vec::new(),
        }
    }

    pub fn semantic(mut self, history: Arc<dyn SemanticHistory>) -> Self {
        self.components.semantic = Some(Arc::new(SemanticRelease::default()));
        self.components.history = Some(history);
        self.components.changelog = Arc::new(GeneratedChangelog);
        self.components.next_version = Arc::new(NextVersion::Disabled);
        self
    }

    pub fn prepare_workflow(&self) -> Result<Workflow<ReleaseRequest, Packaged>> {
        let components = Arc::new(self.components.clone());
        Workflow::start()
            .then(
                self.hooks
                    .capture
                    .clone()
                    .wrap(CaptureStep(components.clone())),
            )?
            .then(
                self.hooks
                    .analyze
                    .clone()
                    .wrap(AnalyzeStep(components.clone())),
            )?
            .then(PrepareStep {
                components: components.clone(),
                hooks: self.hooks.prepare.clone(),
            })?
            .then(
                self.hooks
                    .validate
                    .clone()
                    .wrap(ValidateStep(components.clone())),
            )?
            .then(
                self.hooks
                    .commit
                    .clone()
                    .wrap(CommitStep(components.clone())),
            )?
            .then(self.hooks.build.clone().wrap(BuildStep(components)))
    }

    pub fn release_workflow(&self) -> Result<Workflow<ReleaseRequest, Completed>> {
        let components = Arc::new(self.components.clone());
        self.prepare_workflow()?
            .then(
                self.hooks
                    .preserve
                    .clone()
                    .wrap(PreserveStep(components.clone())),
            )?
            .then(
                self.hooks
                    .publish
                    .clone()
                    .wrap(PublishStep(components.clone())),
            )?
            .then(self.hooks.push.clone().wrap(PushStep(components.clone())))?
            .then(self.hooks.host.clone().wrap(HostStep(components.clone())))?
            .then(NextStep {
                components: components.clone(),
                hooks: self.hooks.next.clone(),
            })?
            .then(self.hooks.verify.clone().wrap(VerifyStep(components)))
    }

    fn options(&self) -> RunOptions {
        let mut observers: Vec<Arc<dyn WorkflowObserver>> =
            vec![Arc::new(Diagnostics(self.components.clone()))];
        observers.extend(self.observers.iter().cloned());
        RunOptions {
            metadata: self.components.execution.metadata(),
            observers,
        }
    }

    pub fn prepare(&self, request: ReleaseRequest) -> Result<Packaged> {
        self.prepare_workflow()?.run_with(request, self.options())
    }

    pub fn release(&self, request: ReleaseRequest) -> Result<Completed> {
        self.release_workflow()?.run_with(request, self.options())
    }
}

fn phase(execution: &ExecutionContext) -> Phase {
    match execution.current_step.as_deref() {
        Some("analyze") => Phase::Analyze,
        Some("prepare") => Phase::Prepare,
        Some("validate") => Phase::Validate,
        Some("commit") => Phase::Commit,
        Some("build") => Phase::Build,
        Some("preserve") => Phase::Preserve,
        Some("publish") => Phase::Publish,
        Some("push") => Phase::Push,
        Some("host") => Phase::Host,
        Some("next") => Phase::Next,
        Some("verify") => Phase::Verify,
        _ => Phase::Capture,
    }
}

fn write_atomic(directory: &Path, name: &str, bytes: &[u8]) -> Result {
    let destination = directory.join(name);
    if fs::symlink_metadata(&destination).is_ok_and(|metadata| !metadata.is_file()) {
        return Err(Error::invalid("release output must be a regular file"));
    }
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(bytes)?;
    temporary
        .persist(destination)
        .map_err(|error| error.error)?;
    Ok(())
}

struct Diagnostics(ReleaseComponents);

impl WorkflowObserver for Diagnostics {
    fn before(&self, execution: &ExecutionContext) -> Result {
        let phase = phase(execution);
        let mut receipt = if phase == Phase::Capture {
            ReleaseReceipt::default()
        } else {
            self.0.store.read()?.unwrap_or_default()
        };
        receipt.phase = phase;
        self.0.store.write(&receipt)?;
        eprintln!("rcr: {}", phase.as_str());
        Ok(())
    }

    fn after(&self, execution: &ExecutionContext) -> Result {
        write_atomic(
            self.0.store.directory(),
            "execution.json",
            &serde_json::to_vec_pretty(execution)?,
        )
    }

    fn failure(&self, execution: &ExecutionContext, error: &Error) -> Result {
        let mut receipt = if phase(execution) == Phase::Capture {
            ReleaseReceipt::default()
        } else {
            self.0.store.read()?.unwrap_or_default()
        };
        receipt.phase = phase(execution);
        receipt.error = Some(error.to_string());
        self.0.store.write(&receipt)?;
        write_atomic(
            self.0.store.directory(),
            "execution.json",
            &serde_json::to_vec_pretty(execution)?,
        )?;
        self.0.execution.summary(&format!(
            "## Release Failed\n\nPhase: `{}`.\n\nSee retained diagnostics; no automatic upload retry was attempted.",
            receipt.phase.as_str()
        ))
    }
}

impl RunData {
    fn persist(&self, components: &ReleaseComponents, phase: Phase) -> Result {
        let mut receipt = ReleaseReceipt::capture(
            &self.release,
            &self.capture.inputs.remote_source,
            self.capture.inputs.source_date_epoch,
        );
        receipt.phase = phase;
        receipt.prepared_tree = self.tree.clone();
        receipt.release_commit = self.commit.clone();
        receipt.next_version = self.next.clone();
        receipt.development_commit = self.development_commit.clone();
        receipt.verification = self.verification.clone();
        if let Some(artifact) = &self.artifact {
            receipt.record_artifact(artifact)?;
        }
        components.store.write(&receipt)
    }

    fn archive(&self) -> Result<&Artifact> {
        self.artifact
            .as_ref()
            .ok_or_else(|| Error::invalid("release archive was not built"))
    }

    fn release_commit(&self) -> Result<&str> {
        self.commit
            .as_deref()
            .ok_or_else(|| Error::invalid("release was not committed"))
    }
}

fn assert_artifact(release: &Release, artifact: &Artifact) -> Result {
    artifact.verify()?;
    if artifact.name() != release.name() || artifact.version() != release.version() {
        return Err(Error::invalid(
            "archive identity differs from captured release",
        ));
    }
    Ok(())
}

fn changelog_path(components: &ReleaseComponents) -> Result<PathBuf> {
    if components.changelog_path.as_os_str().is_empty()
        || components
            .changelog_path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Error::invalid(
            "changelog must be a relative path inside the Git root",
        ));
    }
    let path = components
        .repository
        .root()
        .join(&components.changelog_path);
    if !dunce::canonicalize(path.parent().unwrap())?.starts_with(components.repository.root())
        || fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.is_file())
    {
        return Err(Error::invalid("changelog must not traverse symbolic links"));
    }
    Ok(path)
}

#[derive(Clone)]
pub struct CaptureStep(pub Arc<ReleaseComponents>);

impl Step<ReleaseRequest> for CaptureStep {
    type Output = Captured;
    fn id(&self) -> &str {
        "capture"
    }
    fn run(&self, request: ReleaseRequest, _: &ExecutionContext) -> Result<Captured> {
        let components = &self.0;
        components.execution.validate()?;
        components.repository.assert_clean()?;
        let changelog = changelog_path(components)?;
        if !changelog.exists() && !components.changelog.allows_missing_file() {
            return Err(Error::invalid("handwritten changelog file is missing"));
        }
        let repository = components.repository.inspect()?;
        if repository.shallow {
            return Err(Error::invalid("release requires full Git history"));
        }
        let package = components.package.metadata()?;
        let inputs = if let Some(inputs) = request.captured {
            if inputs.name != package.name
                || inputs.version_override != request.version
                || inputs.source_date_epoch
                    != components.repository.timestamp(&inputs.source_commit)?
                || !components
                    .repository
                    .contains(&inputs.source_commit, &repository.head)?
                || !components
                    .repository
                    .contains(&inputs.remote_source, &repository.head)?
            {
                return Err(Error::invalid(
                    "captured source inputs do not match this checkout or invocation",
                ));
            }
            inputs
        } else {
            if repository.head != repository.remote_head {
                return Err(Error::invalid(
                    "release checkout must match its remote before preparation",
                ));
            }
            CapturedInputs {
                name: package.name,
                manifest_version: package.version.clone(),
                source_commit: repository.head.clone(),
                remote_source: repository.remote_head.clone(),
                source_date_epoch: repository.timestamp,
                date: components.clock.date()?,
                version_override: request.version,
                semantic: None,
            }
        };
        let archive = request.archive.filter(|path| !path.as_os_str().is_empty());
        Ok(Captured {
            inputs,
            repository,
            actual_version: package.version,
            archive,
        })
    }
}

#[derive(Clone)]
pub struct AnalyzeStep(pub Arc<ReleaseComponents>);

impl Step<Captured> for AnalyzeStep {
    type Output = Analyzed;
    fn id(&self) -> &str {
        "analyze"
    }
    fn run(&self, mut capture: Captured, _: &ExecutionContext) -> Result<Analyzed> {
        let components = &self.0;
        let version = if let Some(semantic) = &components.semantic {
            let decision = if let Some(decision) = &capture.inputs.semantic {
                decision.clone()
            } else {
                semantic.analyze(
                    &capture.inputs.source_commit,
                    &capture.inputs.manifest_version,
                    capture.inputs.version_override.as_ref(),
                    &components
                        .history
                        .as_ref()
                        .ok_or_else(|| Error::invalid("semantic history provider is required"))?
                        .read(&capture.inputs.source_commit)?,
                )?
            };
            let (version, _) = decision.require_release()?;
            let version = version.clone();
            decision.check(
                &capture.inputs.source_commit,
                &capture.inputs.manifest_version,
                Some(&version),
                capture.inputs.version_override.as_ref(),
            )?;
            capture.inputs.semantic = Some(decision);
            version
        } else {
            if capture.inputs.semantic.is_some() {
                return Err(Error::invalid(
                    "semantic inputs cannot be used with a numeric workflow",
                ));
            }
            capture
                .inputs
                .version_override
                .clone()
                .unwrap_or_else(|| capture.inputs.manifest_version.clone())
        };
        let release = Release::new(
            &capture.inputs.name,
            version,
            capture.inputs.manifest_version.clone(),
            &capture.inputs.source_commit,
            &capture.inputs.date,
        )?;
        let next = components.next_version.next(&release)?;
        let existing = components.repository.local_tag(release.tag())?;
        if components
            .repository
            .remote_tag(release.tag())?
            .is_some_and(|remote| Some(remote) != existing)
        {
            return Err(Error::invalid(
                "remote release tag conflicts with local release identity",
            ));
        }
        if capture.repository.remote_head != capture.inputs.remote_source
            && !components.repository.contains(
                existing.as_deref().unwrap_or(&capture.repository.head),
                &capture.repository.remote_head,
            )?
        {
            return Err(Error::invalid(
                "remote changed concurrently before preparation",
            ));
        }
        let data = RunData {
            capture,
            release,
            next,
            notes: String::new(),
            tree: None,
            commit: None,
            artifact: None,
            development_commit: None,
            verification: Verification::Pending,
            development: false,
        };
        data.persist(components, Phase::Analyze)?;
        Ok(Analyzed(data))
    }
}

#[derive(Clone)]
struct EditStep(Arc<ReleaseComponents>);

impl Step<Analyzed> for EditStep {
    type Output = Edited;
    fn id(&self) -> &str {
        "prepare"
    }
    fn run(&self, input: Analyzed, _: &ExecutionContext) -> Result<Edited> {
        let mut data = input.0;
        let components = &self.0;
        let existing = components.repository.local_tag(data.release.tag())?;
        data.development =
            existing.is_some() && data.next.as_ref() == Some(&data.capture.actual_version);
        if !data.development {
            if data.capture.actual_version != data.capture.inputs.manifest_version
                && data.capture.actual_version != *data.release.version()
            {
                return Err(Error::invalid(
                    "working manifest conflicts with release inputs",
                ));
            }
            if components.semantic.is_none()
                && data.release.version() < &data.capture.inputs.manifest_version
            {
                return Err(Error::invalid(
                    "numeric release cannot decrease its source manifest version",
                ));
            }
            components.package.set_version(data.release.version())?;
        }
        let path = changelog_path(components)?;
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };
        let generated = data
            .capture
            .inputs
            .semantic
            .as_ref()
            .map(|decision| decision.require_release().map(|(_, notes)| notes))
            .transpose()?;
        let edit = components.changelog.release(
            &source,
            data.release.version(),
            data.release.date(),
            generated,
        )?;
        if let Some(decision) = &data.capture.inputs.semantic {
            decision.validate_prepared(
                data.release.source_commit(),
                data.release.version(),
                &edit.notes,
            )?;
        }
        if edit.source != source {
            if data.development {
                return Err(Error::invalid(
                    "existing development state conflicts with released changelog",
                ));
            }
            fs::write(path, &edit.source)?;
        }
        data.notes = edit.notes;
        Ok(Edited(data))
    }
}

#[derive(Clone)]
pub struct PrepareStep {
    pub components: Arc<ReleaseComponents>,
    pub hooks: LifecycleHooks<Analyzed, Edited>,
}

impl Step<Analyzed> for PrepareStep {
    type Output = Prepared;
    fn id(&self) -> &str {
        "prepare"
    }
    fn run(&self, input: Analyzed, execution: &ExecutionContext) -> Result<Prepared> {
        let components = &self.components;
        components.repository.assert_clean()?;
        if components.repository.inspect()?.head != input.0.capture.repository.head
            || components.package.metadata()?.version != input.0.capture.actual_version
        {
            return Err(Error::invalid("source changed after capture"));
        }
        let mut data = self
            .hooks
            .clone()
            .wrap(EditStep(components.clone()))
            .run(input, execution)?
            .0;
        if components.repository.inspect()?.head != data.capture.repository.head {
            return Err(Error::invalid("preparation hooks must not move HEAD"));
        }
        let package = components.package.metadata()?;
        let expected = if data.development {
            &data.capture.actual_version
        } else {
            data.release.version()
        };
        if package.name != data.release.name() || &package.version != expected {
            return Err(Error::invalid(
                "preparation hook changed captured package identity",
            ));
        }
        let source = fs::read_to_string(changelog_path(components)?)?;
        let generated = data
            .capture
            .inputs
            .semantic
            .as_ref()
            .map(|decision| decision.require_release().map(|(_, notes)| notes))
            .transpose()?;
        let edit = components.changelog.release(
            &source,
            data.release.version(),
            data.release.date(),
            generated,
        )?;
        if edit.source != source {
            return Err(Error::invalid(
                "preparation hook left inconsistent release metadata",
            ));
        }
        data.notes = edit.notes;
        if let Some(decision) = &data.capture.inputs.semantic {
            decision.validate_prepared(
                data.release.source_commit(),
                data.release.version(),
                &data.notes,
            )?;
        }
        data.tree = Some(components.repository.stage_changes()?);
        let existing = components.repository.local_tag(data.release.tag())?;
        if let Some(commit) = existing
            && (data.tree.as_deref() != Some(&data.capture.repository.tree)
                || (!data.development && commit != data.capture.repository.head)
                || !components
                    .repository
                    .contains(&commit, &data.capture.repository.head)?)
        {
            return Err(Error::invalid(
                "prepared source conflicts with immutable release tag",
            ));
        }
        data.persist(components, Phase::Prepare)?;
        Ok(Prepared(data))
    }
}

#[derive(Clone)]
pub struct ValidateStep(pub Arc<ReleaseComponents>);

impl Step<Prepared> for ValidateStep {
    type Output = Validated;
    fn id(&self) -> &str {
        "validate"
    }
    fn run(&self, input: Prepared, execution: &ExecutionContext) -> Result<Validated> {
        let data = input.0;
        let components = &self.0;
        components.validator.validate(
            ValidationInput {
                release: &data.release,
                stage: if data.development {
                    ValidationStage::Development
                } else {
                    ValidationStage::Source
                },
                package: components.package.as_ref(),
                archive: data.capture.archive.as_deref(),
            },
            execution,
        )?;
        components
            .repository
            .assert_prepared(data.tree.as_deref().unwrap())?;
        data.persist(components, Phase::Validate)?;
        Ok(Validated(data))
    }
}

#[derive(Clone)]
pub struct CommitStep(pub Arc<ReleaseComponents>);

impl Step<Validated> for CommitStep {
    type Output = Committed;
    fn id(&self) -> &str {
        "commit"
    }
    fn run(&self, input: Validated, _: &ExecutionContext) -> Result<Committed> {
        let mut data = input.0;
        let components = &self.0;
        components
            .repository
            .assert_prepared(data.tree.as_deref().unwrap())?;
        data.commit = Some(
            if let Some(existing) = components.repository.local_tag(data.release.tag())? {
                existing
            } else {
                components.repository.commit(
                    data.tree.as_deref().unwrap(),
                    &format!("Release {}", data.release.version()),
                )?
            },
        );
        data.persist(components, Phase::Commit)?;
        Ok(Committed(data))
    }
}

#[derive(Clone)]
pub struct BuildStep(pub Arc<ReleaseComponents>);

impl Step<Committed> for BuildStep {
    type Output = Packaged;
    fn id(&self) -> &str {
        "build"
    }
    fn run(&self, input: Committed, _: &ExecutionContext) -> Result<Packaged> {
        let mut data = input.0;
        let components = &self.0;
        components.repository.assert_clean()?;
        let directory = components.store.directory();
        let path = directory.join(format!(
            "{}-{}.crate",
            data.release.name(),
            data.release.version()
        ));
        let artifact = if let Some(override_path) = &data.capture.archive {
            let provided = Artifact::open(override_path)?;
            assert_artifact(&data.release, &provided)?;
            let bytes = provided.validated_bytes()?;
            if path.exists() {
                if fs::read(&path)? != bytes {
                    return Err(Error::invalid(
                        "archive override conflicts with retained artifact",
                    ));
                }
            } else {
                let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
                temporary.write_all(&bytes)?;
                temporary
                    .persist_noclobber(&path)
                    .map_err(|error| error.error)?;
            }
            Artifact::open(&path)?
        } else if path.exists() {
            Artifact::open(&path)?
        } else {
            if data.development {
                return Err(Error::invalid(
                    "cannot rebuild an old release from the advanced manifest",
                ));
            }
            components
                .package
                .build(directory, data.capture.inputs.source_date_epoch)?
        };
        assert_artifact(&data.release, &artifact)?;
        if artifact.vcs_commit()?.as_deref() != Some(data.release_commit()?) {
            return Err(Error::invalid(
                "archive VCS identity differs from prepared release commit",
            ));
        }
        components.package.verify_consumer(&artifact)?;
        components.repository.assert_clean()?;
        components
            .repository
            .ensure_tag(data.release.tag(), data.release_commit()?)?;
        components.repository.preserve_bundle(
            &directory.join(format!(
                "{}-{}.bundle",
                data.release.name(),
                data.release.version()
            )),
            data.release.tag(),
            data.release_commit()?,
        )?;
        data.artifact = Some(artifact);
        data.persist(components, Phase::Build)?;
        Ok(Packaged(data))
    }
}

macro_rules! operation {
    ($name:ident, $input:ident, $output:ident, $id:literal, $phase:ident, [$($mutable:tt)*], |$components:ident, $data:ident, $execution:ident| $body:block $(, skipped = $skip:expr)?) => {
        #[derive(Clone)]
        pub struct $name(pub Arc<ReleaseComponents>);
        impl Step<$input> for $name {
            type Output = $output;
            fn id(&self) -> &str { $id }
            fn run(&self, input: $input, $execution: &ExecutionContext) -> Result<$output> {
                let $($mutable)* $data = input.0;
                let $components = &self.0;
                $body
                $data.persist($components, Phase::$phase)?;
                Ok($output($data))
            }
            $(fn outcome(&self, output: &Self::Output) -> StepOutcome {
                if ($skip)(output, self.0.as_ref()) { StepOutcome::Skipped } else { StepOutcome::Success }
            })?
        }
    };
}

operation!(
    PreserveStep,
    Packaged,
    Preserved,
    "preserve",
    Preserve,
    [],
    |components, data, _execution| {
        data.persist(components, Phase::Preserve)?;
        components.repository.preserve_bundle(
            &components.store.directory().join(format!(
                "{}-{}.bundle",
                data.release.name(),
                data.release.version()
            )),
            data.release.tag(),
            data.release_commit()?,
        )?;
        components.preserver.preserve(
            &data.release,
            components.store.directory(),
            components.execution.as_ref(),
        )?;
    }
);

operation!(
    PublishStep,
    Preserved,
    Published,
    "publish",
    Publish,
    [],
    |components, data, _execution| {
        assert_artifact(&data.release, data.archive()?)?;
        components.repository.assert_clean()?;
        if components
            .repository
            .local_tag(data.release.tag())?
            .as_deref()
            != Some(data.release_commit()?)
            || components
                .repository
                .remote_tag(data.release.tag())?
                .is_some_and(|commit| Some(commit.as_str()) != data.commit.as_deref())
        {
            return Err(Error::invalid("release tag changed before publication"));
        }
        let remote = components.repository.inspect()?.remote_head;
        if remote != data.capture.inputs.remote_source
            && !components
                .repository
                .contains(data.release_commit()?, &remote)?
        {
            return Err(Error::invalid(
                "remote changed concurrently before publication",
            ));
        }
        components
            .publisher
            .publish(data.archive()?, components.execution.as_ref())?;
    }
);

operation!(
    PushStep,
    Published,
    Pushed,
    "push",
    Push,
    [],
    |components, data, _execution| {
        components.repository.push_release(
            data.release_commit()?,
            data.release.tag(),
            &data.capture.inputs.remote_source,
        )?;
    }
);

operation!(
    HostStep,
    Pushed,
    Hosted,
    "host",
    Host,
    [],
    |components, data, _execution| {
        if let Some(host) = &components.host {
            host.publish(
                &data.release,
                data.release_commit()?,
                &data.notes,
                data.archive()?,
            )?;
        }
    },
    skipped = |_: &Hosted, components: &ReleaseComponents| components.host.is_none()
);

#[derive(Clone)]
struct EditNextStep(Arc<ReleaseComponents>);

impl Step<Hosted> for EditNextStep {
    type Output = NextEdited;
    fn id(&self) -> &str {
        "next"
    }
    fn run(&self, input: Hosted, _: &ExecutionContext) -> Result<NextEdited> {
        let data = input.0;
        let components = &self.0;
        if let Some(next) = &data.next {
            let current = components.package.metadata()?.version;
            if current == *data.release.version() {
                components.package.set_version(next)?;
                let path = changelog_path(components)?;
                let source = fs::read_to_string(&path)?;
                let updated = components
                    .changelog
                    .next(&source, data.release.version(), next)?;
                if source != updated {
                    fs::write(path, updated)?;
                }
            } else if current != *next {
                return Err(Error::invalid(
                    "development manifest conflicts with captured next version",
                ));
            }
        }
        Ok(NextEdited(data))
    }
}

#[derive(Clone)]
pub struct NextStep {
    pub components: Arc<ReleaseComponents>,
    pub hooks: LifecycleHooks<Hosted, NextEdited>,
}

impl Step<Hosted> for NextStep {
    type Output = Development;
    fn id(&self) -> &str {
        "next"
    }
    fn outcome(&self, output: &Development) -> StepOutcome {
        if output.0.next.is_none() {
            StepOutcome::Skipped
        } else {
            StepOutcome::Success
        }
    }
    fn run(&self, input: Hosted, execution: &ExecutionContext) -> Result<Development> {
        let components = &self.components;
        components.repository.assert_clean()?;
        let head = components.repository.inspect()?.head;
        let mut data = self
            .hooks
            .clone()
            .wrap(EditNextStep(components.clone()))
            .run(input, execution)?
            .0;
        if components.repository.inspect()?.head != head {
            return Err(Error::invalid("next hooks must not move HEAD"));
        }
        if let Some(next) = &data.next {
            let package = components.package.metadata()?;
            if package.name != data.release.name() || &package.version != next {
                return Err(Error::invalid(
                    "next hook changed captured package identity",
                ));
            }
            let source = fs::read_to_string(changelog_path(components)?)?;
            let edit = components.changelog.release(
                &source,
                data.release.version(),
                data.release.date(),
                Some(&data.notes),
            )?;
            if edit.source != source
                || components
                    .changelog
                    .next(&source, data.release.version(), next)?
                    != source
            {
                return Err(Error::invalid(
                    "next hook changed immutable release notes or pending metadata",
                ));
            }
            let tree = components.repository.stage_changes()?;
            components.validator.validate(
                ValidationInput {
                    release: &data.release,
                    stage: ValidationStage::Development,
                    package: components.package.as_ref(),
                    archive: None,
                },
                execution,
            )?;
            components.repository.assert_prepared(&tree)?;
            let commit = components
                .repository
                .commit(&tree, &format!("Prepare {} development", next))?;
            data.development_commit = Some(commit.clone());
            data.persist(components, Phase::Next)?;
            components
                .repository
                .push_development(&commit, data.release_commit()?)?;
        } else {
            components.repository.assert_clean()?;
        }
        data.persist(components, Phase::Next)?;
        Ok(Development(data))
    }
}

operation!(VerifyStep, Development, Completed, "verify", Complete, [mut], |components, data, _execution| {
    components.repository.assert_clean()?;
    data.verification = components.verifier.verify(&data.release, data.archive()?, components.publisher.as_ref())?;
    data.persist(components, Phase::Verify)?;
    let status = match data.verification {
        Verification::Verified { .. } => "Verified",
        Verification::NotVisible { .. } => "Release could not be verified (registry visibility timed out)",
        Verification::Pending => return Err(Error::invalid("verification provider returned pending")),
    };
    components.execution.summary(&format!("## Release {}\n\nPackage: `{}`.\n\n{status}.", data.release.version(), data.release.name()))?;
});

#[cfg(test)]
mod tests;
