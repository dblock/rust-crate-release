use super::*;
use crate::changelog::IntrideaChangelog;
use crate::git::{Git, GitHistory};
use crate::package::{CargoPackage, PackageMetadata};
use crate::registry::{PublishOutcome, PublishedCrate};
use crate::store::DirectoryReleaseStore;
use crate::testing::{DirectoryReleaseHost, GitProject, LocalRegistry};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct FixedClock;
impl ReleaseClock for FixedClock {
    fn date(&self) -> Result<String> {
        Ok("2026/11/11".into())
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<ExecutionContext>>);
impl WorkflowObserver for Events {
    fn after(&self, execution: &ExecutionContext) -> Result {
        self.0.lock().unwrap().push(execution.clone());
        Ok(())
    }
}

struct Fixture {
    project: GitProject,
    git: Arc<Git>,
    package: Arc<CargoPackage>,
    publisher: Arc<LocalRegistry>,
    host: Arc<DirectoryReleaseHost>,
    store: Arc<DirectoryReleaseStore>,
    engine: ReleaseWorkflow,
}

impl Fixture {
    fn new() -> Result<Self> {
        let project = GitProject::cargo_library()?;
        let git = Arc::new(Git::new(project.root(), "master", "origin")?);
        let package = Arc::new(CargoPackage::new(project.root())?);
        let publisher = Arc::new(LocalRegistry::new(
            project.temporary_directory().join("registry"),
        )?);
        let host = Arc::new(DirectoryReleaseHost::new(
            project.temporary_directory().join("host"),
        )?);
        let store = Arc::new(DirectoryReleaseStore::new(git.artifact_directory()?)?);
        let mut engine = ReleaseWorkflow::new(
            git.clone(),
            package.clone(),
            publisher.clone(),
            store.clone(),
        );
        engine.components.changelog = Arc::new(IntrideaChangelog::default());
        engine.components.host = Some(host.clone());
        engine.components.clock = Arc::new(FixedClock);
        engine.components.verifier = Arc::new(RegistryVerifier::new(3, Duration::ZERO)?);
        Ok(Self {
            project,
            git,
            package,
            publisher,
            host,
            store,
            engine,
        })
    }

    fn request(prepared: &Packaged) -> ReleaseRequest {
        ReleaseRequest {
            captured: Some(prepared.captured().clone()),
            ..ReleaseRequest::default()
        }
    }
}

#[test]
fn numeric_release_reconciles_real_outputs_and_verifies_after_manifest_advances() -> Result {
    let mut fixture = Fixture::new()?;
    let events = Arc::new(Events::default());
    fixture.engine.observers.push(events.clone());
    fixture.store.write(&ReleaseReceipt {
        phase: Phase::Complete,
        ..ReleaseReceipt::default()
    })?;
    let original = fixture.git.inspect()?.head;
    let prepared = fixture.engine.prepare(ReleaseRequest::default())?;
    assert_eq!(fixture.publisher.publication_count(), 0);
    assert_eq!(fixture.git.remote_head()?, original);
    assert_eq!(
        prepared.artifact().unwrap().vcs_commit()?.as_deref(),
        prepared.commit()
    );
    let request = Fixture::request(&prepared);
    let output = fixture.engine.release(request.clone())?;
    let next_head = fixture.git.inspect()?.head;
    assert_ne!(next_head, output.commit().unwrap());
    assert_eq!(fixture.package.metadata()?.version, Version::new(0, 1, 1));
    assert!(
        fs::read_to_string(fixture.project.root().join("Cargo.lock"))?
            .contains("version = \"0.1.1\"")
    );
    assert_eq!(fixture.publisher.publication_count(), 1);
    assert_eq!(fixture.host.publication_count(), 1);
    assert!(matches!(
        output.verification(),
        Verification::Verified { .. }
    ));
    let again = fixture.engine.release(request)?;
    assert_eq!(again.commit(), output.commit());
    assert_eq!(fixture.git.inspect()?.head, next_head);
    assert_eq!(fixture.publisher.publication_count(), 1);
    assert_eq!(fixture.host.publication_count(), 1);
    let archive = output.artifact().unwrap();
    assert_eq!(
        fixture.engine.components.verifier.verify(
            output.release(),
            archive,
            fixture.publisher.as_ref()
        )?,
        *output.verification(),
    );
    let receipt = fixture.store.read()?.unwrap();
    assert_eq!(receipt.phase, Phase::Complete);
    assert_eq!(receipt.release.unwrap().version, Version::new(0, 1, 0));
    let events = events.0.lock().unwrap();
    let history = &events.last().unwrap().history;
    assert_eq!(
        history
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        [
            "capture", "analyze", "prepare", "validate", "commit", "build", "preserve", "publish",
            "push", "host", "next", "verify"
        ],
    );
    Ok(())
}

#[test]
fn disabled_changelog_preserves_existing_files_and_releases_without_any_path() -> Result {
    use crate::changelog::NoChangelog;

    for semantic in [false, true] {
        let mut fixture = Fixture::new()?;
        let path = fixture.project.root().join("CHANGELOG.md");
        if semantic {
            fs::remove_file(&path)?;
            fixture.engine = fixture
                .engine
                .semantic(Arc::new(GitHistory::new((*fixture.git).clone())));
            fixture.engine.components.next_version = Arc::new(NextVersion::Patch);
            fixture.engine.components.changelog_path = "absent-parent/CHANGELOG.md".into();
        } else {
            fs::write(&path, "Not a valid changelog.\n")?;
        }
        let original = fs::read(&path).ok();
        let tree = fixture.git.stage_changes()?;
        fixture
            .git
            .commit(&tree, "feat: release without a changelog")?;
        command::run(fixture.project.root(), "git", &["push", "origin", "master"])?;
        fixture.engine.components.changelog = Arc::new(NoChangelog);
        let output = fixture.engine.release(ReleaseRequest::default())?;
        assert_eq!(fs::read(&path).ok(), original);
        assert!(output.notes().contains(if semantic {
            "release without a changelog"
        } else {
            "Release 0.1.0."
        }));
        assert!(matches!(
            output.verification(),
            Verification::Verified { .. }
        ));
        assert!(output.development_commit().is_some());
        fixture.git.assert_clean()?;
        fixture.engine.release(ReleaseRequest {
            captured: Some(output.captured().clone()),
            ..ReleaseRequest::default()
        })?;
        assert_eq!(fixture.publisher.publication_count(), 1);
        assert_eq!(fs::read(&path).ok(), original);
        NoChangelog.release_file(
            fixture.project.root(),
            &Version::new(0, 1, 0),
            "2026/11/11",
            None,
        )?;
        NoChangelog.next_file(
            fixture.project.root(),
            &Version::new(0, 1, 0),
            &Version::new(0, 1, 1),
        )?;
    }
    Ok(())
}

#[test]
fn automatic_default_releases_existing_changelog_and_reconciles_development() -> Result {
    let fixture = Fixture::new()?;
    let engine = ReleaseWorkflow::new(
        fixture.git.clone(),
        fixture.package.clone(),
        fixture.publisher.clone(),
        fixture.store.clone(),
    );
    let output = engine.release(ReleaseRequest::default())?;
    assert_eq!(output.notes(), "- Initial fixture.");
    let path = fixture.project.root().join("CHANGELOG.md");
    let next = fs::read_to_string(&path)?;
    assert!(next.contains("## 0.1.1 (Next)"));
    engine.release(ReleaseRequest {
        captured: Some(output.captured().clone()),
        ..ReleaseRequest::default()
    })?;
    assert_eq!(fs::read_to_string(path)?, next);
    assert_eq!(fixture.publisher.publication_count(), 1);
    fixture.git.assert_clean()?;
    Ok(())
}

struct EditHook(PathBuf);

struct FineHook {
    name: &'static str,
    events: Arc<Mutex<Vec<String>>>,
    fail_after: bool,
}

impl<I, O> crate::hooks::LifecycleHook<I, O> for FineHook {
    fn id(&self) -> &str {
        self.name
    }
    fn before(&self, _: &I, _: &ExecutionContext) -> Result {
        self.events
            .lock()
            .unwrap()
            .push(format!("{} before", self.name));
        Ok(())
    }
    fn after(&self, _: &O, _: &ExecutionContext) -> Result {
        self.events
            .lock()
            .unwrap()
            .push(format!("{} after", self.name));
        if self.fail_after {
            return Err(Error::invalid("fine-grained hook failed"));
        }
        Ok(())
    }
}

#[test]
fn fine_grained_hooks_follow_real_tag_authentication_and_development_boundaries() -> Result {
    let mut fixture = Fixture::new()?;
    let events = Arc::new(Mutex::new(Vec::new()));
    let hook = |name| FineHook {
        name,
        events: events.clone(),
        fail_after: false,
    };
    fixture.engine.hooks.tag = fixture.engine.hooks.tag.with(hook("tag"));
    fixture.engine.hooks.authenticate =
        fixture.engine.hooks.authenticate.with(hook("authenticate"));
    fixture.engine.hooks.commit_next = fixture.engine.hooks.commit_next.with(hook("commit_next"));
    fixture.engine.hooks.push_next = fixture.engine.hooks.push_next.with(hook("push_next"));
    fixture.engine.release(ReleaseRequest::default())?;
    assert_eq!(
        *events.lock().unwrap(),
        [
            "tag before",
            "tag after",
            "authenticate before",
            "authenticate after",
            "commit_next before",
            "commit_next after",
            "push_next before",
            "push_next after",
        ]
    );
    assert_eq!(fixture.publisher.publication_count(), 1);
    fixture.git.assert_clean()?;

    for authenticate in [true, false] {
        let mut fixture = Fixture::new()?;
        let hook = FineHook {
            name: "blocked",
            events: Arc::new(Mutex::new(Vec::new())),
            fail_after: true,
        };
        if authenticate {
            fixture.engine.hooks.authenticate = fixture.engine.hooks.authenticate.with(hook);
        } else {
            fixture.engine.hooks.commit_next = fixture.engine.hooks.commit_next.with(hook);
        }
        let source = fixture.git.remote_head()?;
        let error = fixture
            .engine
            .release(ReleaseRequest::default())
            .unwrap_err();
        assert!(error.to_string().contains("blocked"));
        assert_eq!(
            fixture.publisher.publication_count(),
            usize::from(!authenticate)
        );
        if authenticate {
            assert_eq!(fixture.git.remote_head()?, source);
        } else {
            assert_ne!(fixture.git.remote_head()?, fixture.git.inspect()?.head);
            assert!(fixture.git.remote_tag("v0.1.0")?.is_some());
        }
    }
    Ok(())
}
impl crate::hooks::LifecycleHook<Analyzed, Edited> for EditHook {
    fn id(&self) -> &str {
        "trusted-readme"
    }
    fn before(&self, _: &Analyzed, _: &ExecutionContext) -> Result {
        fs::write(self.0.join("README.md"), "Prepared by trusted hook.\n")?;
        Ok(())
    }
    fn after(&self, _: &Edited, _: &ExecutionContext) -> Result {
        fs::write(
            self.0.join("README.md"),
            "Prepared by trusted after hook.\n",
        )?;
        Ok(())
    }
}

struct NextEditHook(PathBuf);
impl crate::hooks::LifecycleHook<Hosted, NextEdited> for NextEditHook {
    fn id(&self) -> &str {
        "development-readme"
    }
    fn after(&self, _: &NextEdited, _: &ExecutionContext) -> Result {
        fs::write(self.0.join("README.md"), "Prepared for development.\n")?;
        Ok(())
    }
}

#[test]
fn after_preparation_and_next_hooks_finish_before_trusted_staging_and_real_validation() -> Result {
    let mut fixture = Fixture::new()?;
    fixture.engine.hooks.prepare = fixture
        .engine
        .hooks
        .prepare
        .with(EditHook(fixture.project.root().into()));
    fixture.engine.hooks.next = fixture
        .engine
        .hooks
        .next
        .with(NextEditHook(fixture.project.root().into()));
    let output = fixture.engine.release(ReleaseRequest::default())?;
    let released = command::text(command::run(
        fixture.project.root(),
        "git",
        &["show", &format!("{}:README.md", output.commit().unwrap())],
    )?)?;
    assert_eq!(released, "Prepared by trusted after hook.");
    assert_eq!(
        fs::read_to_string(fixture.project.root().join("README.md"))?,
        "Prepared for development.\n"
    );
    fixture.git.assert_clean()?;
    assert_eq!(fixture.git.remote_head()?, fixture.git.inspect()?.head);
    Ok(())
}

struct TamperValidator;
impl ReleaseValidator for TamperValidator {
    fn validate(&self, input: ValidationInput<'_>, execution: &ExecutionContext) -> Result {
        let root = input.package.root().to_path_buf();
        CommandValidator::default().validate(input, execution)?;
        fs::write(root.join("README.md"), "Unexpected validator edit.\n")?;
        Ok(())
    }
}

#[test]
fn trusted_hook_edits_are_staged_but_validator_changes_are_preserved_and_rejected() -> Result {
    let mut fixture = Fixture::new()?;
    let original = fixture.git.inspect()?.head;
    fixture.engine.hooks.prepare = fixture
        .engine
        .hooks
        .prepare
        .with(EditHook(fixture.project.root().into()));
    fixture.engine.components.validator = Arc::new(TamperValidator);
    let error = fixture
        .engine
        .prepare(ReleaseRequest::default())
        .unwrap_err();
    assert!(error.to_string().contains("validation changed"));
    assert_eq!(fixture.git.inspect()?.head, original);
    assert_eq!(fixture.git.local_tag("v0.1.0")?, None);
    assert_eq!(fixture.publisher.publication_count(), 0);
    assert_eq!(
        fs::read_to_string(fixture.project.root().join("README.md"))?,
        "Unexpected validator edit.\n"
    );
    let receipt = fixture.store.read()?.unwrap();
    assert_eq!(receipt.phase, Phase::Validate);
    assert!(receipt.error.unwrap().contains("validation changed"));
    Ok(())
}

struct RejectPreservation;
impl ArtifactPreserver for RejectPreservation {
    fn preserve(&self, release: &Release, directory: &Path, _: &dyn ExecutionProvider) -> Result {
        assert!(
            directory
                .join(format!("{}-{}.crate", release.name(), release.version()))
                .is_file()
        );
        assert!(
            directory
                .join(format!("{}-{}.bundle", release.name(), release.version()))
                .is_file()
        );
        assert!(directory.join("release.json").is_file());
        assert!(directory.join("execution.json").is_file());
        Err(Error::invalid("durable upload failed"))
    }
}

#[test]
fn failed_durable_preservation_prevents_publication_and_retry_reconciles_local_artifacts() -> Result
{
    let mut fixture = Fixture::new()?;
    let prepared = fixture.engine.prepare(ReleaseRequest::default())?;
    let request = Fixture::request(&prepared);
    fixture.engine.components.preserver = Arc::new(RejectPreservation);
    assert!(
        fixture
            .engine
            .release(request.clone())
            .unwrap_err()
            .to_string()
            .contains("durable upload failed")
    );
    assert_eq!(fixture.publisher.publication_count(), 0);
    assert_eq!(fixture.host.publication_count(), 0);
    assert_eq!(fixture.git.remote_tag("v0.1.0")?, None);
    fixture.engine.components.preserver = Arc::new(LocalArtifacts);
    fixture.engine.release(request)?;
    assert_eq!(fixture.publisher.publication_count(), 1);
    Ok(())
}

#[cfg(unix)]
#[test]
fn failed_atomic_push_is_recoverable_without_republishing_or_moving_the_tag() -> Result {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new()?;
    let prepared = fixture.engine.prepare(ReleaseRequest::default())?;
    let request = Fixture::request(&prepared);
    let hooks = fixture.project.temporary_directory().join("reject-push");
    fs::create_dir(&hooks)?;
    let hook = hooks.join("pre-receive");
    fs::write(&hook, "#!/bin/sh\nexit 1\n")?;
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))?;
    command::run(
        fixture.project.remote(),
        "git",
        &[
            "--git-dir",
            fixture.project.remote().to_str().unwrap(),
            "config",
            "core.hooksPath",
            hooks.to_str().unwrap(),
        ],
    )?;
    assert!(fixture.engine.release(request.clone()).is_err());
    assert_eq!(fixture.publisher.publication_count(), 1);
    assert_eq!(fixture.host.publication_count(), 0);
    assert_eq!(fixture.git.remote_tag("v0.1.0")?, None);
    assert_eq!(fixture.store.read()?.unwrap().phase, Phase::Push);
    fs::write(hook, "#!/bin/sh\nexit 0\n")?;
    let output = fixture.engine.release(request)?;
    assert_eq!(fixture.publisher.publication_count(), 1);
    assert_eq!(fixture.host.publication_count(), 1);
    assert_eq!(
        fixture.git.remote_tag("v0.1.0")?.as_deref(),
        output.commit()
    );
    Ok(())
}

struct FailNextOnce(AtomicBool);
impl ReleaseValidator for FailNextOnce {
    fn validate(&self, input: ValidationInput<'_>, execution: &ExecutionContext) -> Result {
        let next = input.stage == ValidationStage::Development;
        CommandValidator::default().validate(input, execution)?;
        if next && self.0.swap(false, Ordering::SeqCst) {
            return Err(Error::invalid("next validation failed"));
        }
        Ok(())
    }
}

#[test]
fn failed_next_validation_retains_accepted_release_and_retries_after_explicit_clean_repair()
-> Result {
    let mut fixture = Fixture::new()?;
    let prepared = fixture.engine.prepare(ReleaseRequest::default())?;
    let request = Fixture::request(&prepared);
    fixture.engine.components.validator = Arc::new(FailNextOnce(AtomicBool::new(true)));
    assert!(
        fixture
            .engine
            .release(request.clone())
            .unwrap_err()
            .to_string()
            .contains("next validation failed")
    );
    assert_eq!(fixture.publisher.publication_count(), 1);
    assert_eq!(fixture.host.publication_count(), 1);
    assert!(fixture.git.remote_tag("v0.1.0")?.is_some());
    assert_eq!(fixture.store.read()?.unwrap().phase, Phase::Next);
    assert!(
        fixture
            .engine
            .release(request.clone())
            .unwrap_err()
            .to_string()
            .contains("clean checkout")
    );
    let tree = fixture.git.stage_changes()?;
    fixture
        .git
        .commit(&tree, "Retain repaired development changes")?;
    fixture.engine.release(request)?;
    assert_eq!(fixture.publisher.publication_count(), 1);
    assert_eq!(fixture.package.metadata()?.version, Version::new(0, 1, 1));
    Ok(())
}

struct DelayedRegistry {
    registry: Arc<LocalRegistry>,
    missing: AtomicUsize,
    fail: bool,
    wrong_bytes: bool,
}
impl PackagePublisher for DelayedRegistry {
    fn lookup(&self, name: &str, version: &Version) -> Result<Option<PublishedCrate>> {
        if self.fail {
            return Err(Error::invalid("registry authentication failed"));
        }
        if self
            .missing
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Ok(None);
        }
        self.registry.lookup(name, version)
    }
    fn publish(
        &self,
        artifact: &Artifact,
        execution: &dyn ExecutionProvider,
    ) -> Result<PublishOutcome> {
        self.registry.publish(artifact, execution)
    }
    fn download(&self, name: &str, version: &Version) -> Result<Vec<u8>> {
        let mut bytes = self.registry.download(name, version)?;
        if self.wrong_bytes {
            bytes.push(0);
        }
        Ok(bytes)
    }
}

#[test]
fn visibility_timeouts_do_not_block_hosting_or_next_and_verification_is_explicit_and_readonly()
-> Result {
    for missing in [2, 3] {
        let mut fixture = Fixture::new()?;
        let registry = Arc::new(DelayedRegistry {
            registry: fixture.publisher.clone(),
            missing: AtomicUsize::new(missing),
            fail: false,
            wrong_bytes: false,
        });
        fixture.engine.components.publisher = registry.clone();
        let output = fixture.engine.release(ReleaseRequest::default())?;
        assert_eq!(fixture.publisher.publication_count(), 1);
        assert_eq!(fixture.host.publication_count(), 1);
        assert_eq!(fixture.package.metadata()?.version, Version::new(0, 1, 1));
        if missing == 3 {
            assert_eq!(
                output.verification(),
                &Verification::NotVisible { attempts: 3 }
            );
        } else {
            assert!(matches!(
                output.verification(),
                Verification::Verified { .. }
            ));
        }
        let head = fixture.git.inspect()?.head;
        assert!(matches!(
            fixture.engine.components.verifier.verify(
                output.release(),
                output.artifact().unwrap(),
                registry.as_ref()
            )?,
            Verification::Verified { .. }
        ));
        assert_eq!(fixture.git.inspect()?.head, head);
        assert_eq!(fixture.publisher.publication_count(), 1);
    }
    Ok(())
}

#[test]
fn verification_errors_and_wrong_download_bytes_are_not_visibility_timeouts() -> Result {
    let fixture = Fixture::new()?;
    let output = fixture.engine.release(ReleaseRequest::default())?;
    for (fail, wrong_bytes) in [(true, false), (false, true)] {
        let publisher = DelayedRegistry {
            registry: fixture.publisher.clone(),
            missing: AtomicUsize::new(0),
            fail,
            wrong_bytes,
        };
        assert!(
            fixture
                .engine
                .components
                .verifier
                .verify(output.release(), output.artifact().unwrap(), &publisher)
                .is_err()
        );
    }
    assert_eq!(fixture.publisher.publication_count(), 1);
    Ok(())
}

struct CountingHistory {
    history: GitHistory,
    reads: AtomicUsize,
}
impl SemanticHistory for CountingHistory {
    fn read(&self, source: &str) -> Result<crate::semantic::SemanticHistoryData> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.history.read(source)
    }
}

#[test]
fn semantic_preparation_captures_analysis_once_and_skips_optional_host_and_next() -> Result {
    let mut fixture = Fixture::new()?;
    fs::write(fixture.project.root().join("CHANGELOG.md"), "# Changelog\n")?;
    let tree = fixture.git.stage_changes()?;
    fixture.git.commit(&tree, "feat: support releases")?;
    command::run(fixture.project.root(), "git", &["push", "origin", "master"])?;
    let history = Arc::new(CountingHistory {
        history: GitHistory::new((*fixture.git).clone()),
        reads: AtomicUsize::new(0),
    });
    fixture.engine = fixture.engine.semantic(history.clone());
    fixture.engine.components.host = None;
    let events = Arc::new(Events::default());
    fixture.engine.observers.push(events.clone());
    let prepared = fixture.engine.prepare(ReleaseRequest::default())?;
    assert_eq!(prepared.release().version(), &Version::new(1, 0, 0));
    let request = Fixture::request(&prepared);
    let output = fixture.engine.release(request.clone())?;
    assert_eq!(history.reads.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.package.metadata()?.version, Version::new(1, 0, 0));
    assert!(output.notes().contains("support releases"));
    let events = events.0.lock().unwrap();
    let records = &events.last().unwrap().history;
    for id in ["host", "next"] {
        assert_eq!(
            records
                .iter()
                .find(|record| record.id == id)
                .unwrap()
                .outcome,
            StepOutcome::Skipped
        );
    }
    drop(events);
    let mut tampered = request;
    let version = Version::new(2, 0, 0);
    tampered.version = Some(version.clone());
    tampered.captured.as_mut().unwrap().version_override = Some(version);
    assert!(fixture.engine.release(tampered).is_err());
    assert_eq!(fixture.publisher.publication_count(), 1);
    Ok(())
}

#[test]
fn no_eligible_semantic_history_fails_before_edits_and_provider_guards_are_fail_closed() -> Result {
    let mut fixture = Fixture::new()?;
    let original = fs::read(fixture.project.root().join("CHANGELOG.md"))?;
    fixture.engine = fixture
        .engine
        .semantic(Arc::new(GitHistory::new((*fixture.git).clone())));
    assert!(fixture.engine.prepare(ReleaseRequest::default()).is_err());
    assert_eq!(
        fs::read(fixture.project.root().join("CHANGELOG.md"))?,
        original
    );
    assert_eq!(fixture.publisher.publication_count(), 0);
    assert!(fixture.git.assert_clean().is_ok());
    assert!(SystemClock::new("Not/A_Timezone").is_err());
    assert!(RegistryVerifier::new(0, Duration::ZERO).is_err());
    Ok(())
}

struct CountingPackage {
    package: Arc<CargoPackage>,
    builds: AtomicUsize,
    consumers: AtomicUsize,
}
impl PackageProject for CountingPackage {
    fn root(&self) -> &Path {
        self.package.root()
    }
    fn metadata(&self) -> Result<PackageMetadata> {
        self.package.metadata()
    }
    fn set_version(&self, version: &Version) -> Result {
        self.package.set_version(version)
    }
    fn build(&self, directory: &Path, epoch: i64) -> Result<Artifact> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        self.package.build(directory, epoch)
    }
    fn verify_consumer(&self, artifact: &Artifact) -> Result {
        self.consumers.fetch_add(1, Ordering::SeqCst);
        self.package.verify_consumer(artifact)
    }
}

#[test]
fn archive_overrides_provided_unset_and_empty_use_the_real_retained_consumer_without_repacking()
-> Result {
    let mut fixture = Fixture::new()?;
    let package = Arc::new(CountingPackage {
        package: fixture.package.clone(),
        builds: AtomicUsize::new(0),
        consumers: AtomicUsize::new(0),
    });
    fixture.engine.components.package = package.clone();
    let prepared = fixture.engine.prepare(ReleaseRequest::default())?;
    for archive in [
        None,
        Some(PathBuf::new()),
        Some(prepared.artifact().unwrap().path().into()),
    ] {
        let mut request = Fixture::request(&prepared);
        request.archive = archive;
        fixture.engine.prepare(request)?;
    }
    assert_eq!(package.builds.load(Ordering::SeqCst), 1);
    assert_eq!(package.consumers.load(Ordering::SeqCst), 4);
    let bad = fixture.project.temporary_directory().join("invalid.crate");
    fs::write(&bad, b"not an archive")?;
    let mut request = Fixture::request(&prepared);
    request.archive = Some(bad);
    assert!(fixture.engine.prepare(request).is_err());
    assert_eq!(package.builds.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.publisher.publication_count(), 0);
    Ok(())
}

#[test]
fn numeric_override_updates_archive_and_next_without_decreasing_source() -> Result {
    let fixture = Fixture::new()?;
    let output = fixture.engine.release(ReleaseRequest {
        version: Some(Version::new(0, 2, 0)),
        ..ReleaseRequest::default()
    })?;
    assert_eq!(output.artifact().unwrap().version(), &Version::new(0, 2, 0));
    assert_eq!(fixture.package.metadata()?.version, Version::new(0, 2, 1));
    let source = fs::read_to_string(fixture.project.root().join("CHANGELOG.md"))?;
    assert!(source.contains("## 0.2.0 (2026/11/11)"));
    assert!(source.contains("## 0.2.1 (Next)"));
    let other = Fixture::new()?;
    let before = fs::read(other.project.root().join("CHANGELOG.md"))?;
    assert!(
        other
            .engine
            .prepare(ReleaseRequest {
                version: Some(Version::new(0, 0, 1)),
                ..ReleaseRequest::default()
            })
            .is_err()
    );
    assert_eq!(fs::read(other.project.root().join("CHANGELOG.md"))?, before);
    other.git.assert_clean()?;
    Ok(())
}

#[test]
fn tampered_inputs_and_shallow_or_escaping_projects_fail_before_edits() -> Result {
    let mut fixture = Fixture::new()?;
    let prepared = fixture.engine.prepare(ReleaseRequest::default())?;
    for kind in ["name", "timestamp", "source", "override"] {
        let mut request = Fixture::request(&prepared);
        let inputs = request.captured.as_mut().unwrap();
        match kind {
            "name" => inputs.name = "different-crate".into(),
            "timestamp" => inputs.source_date_epoch += 1,
            "source" => inputs.source_commit = "f".repeat(40),
            "override" => request.version = Some(Version::new(0, 2, 0)),
            _ => unreachable!(),
        }
        assert!(fixture.engine.prepare(request).is_err());
        fixture.git.assert_clean()?;
    }
    fixture.engine.components.changelog_path = "../outside.md".into();
    assert!(fixture.engine.prepare(Fixture::request(&prepared)).is_err());
    fixture.git.assert_clean()?;
    fixture.engine.components.changelog_path = "CHANGELOG.md".into();
    let clone = fixture.project.temporary_directory().join("shallow");
    let url = reqwest::Url::from_file_path(fixture.project.remote())
        .unwrap()
        .to_string();
    command::run(
        fixture.project.temporary_directory(),
        "git",
        &[
            "clone",
            "--depth",
            "1",
            "--branch",
            "master",
            &url,
            clone.to_str().unwrap(),
        ],
    )?;
    fixture.engine.components.repository = Arc::new(Git::new(&clone, "master", "origin")?);
    fixture.engine.components.package = Arc::new(CargoPackage::new(&clone)?);
    assert!(
        fixture
            .engine
            .prepare(ReleaseRequest::default())
            .unwrap_err()
            .to_string()
            .contains("full Git history")
    );
    assert_eq!(fixture.publisher.publication_count(), 0);
    Ok(())
}

#[test]
fn same_version_archive_from_another_vcs_commit_is_never_published() -> Result {
    let fixture = Fixture::new()?;
    let prepared = fixture.engine.prepare(ReleaseRequest::default())?;
    let other = Fixture::new()?;
    let archive = other
        .package
        .build(other.store.directory(), other.git.inspect()?.timestamp)?;
    assert_ne!(archive.vcs_commit()?.as_deref(), prepared.commit());
    fs::remove_file(prepared.artifact().unwrap().path())?;
    let mut request = Fixture::request(&prepared);
    request.archive = Some(archive.path().into());
    assert!(
        fixture
            .engine
            .release(request)
            .unwrap_err()
            .to_string()
            .contains("VCS identity")
    );
    assert_eq!(fixture.publisher.publication_count(), 0);
    assert_eq!(fixture.git.remote_tag("v0.1.0")?, None);
    Ok(())
}

#[test]
fn github_without_a_real_durable_uploader_fails_before_publication() -> Result {
    let mut fixture = Fixture::new()?;
    let values = [
        (
            "GITHUB_EVENT_NAME".to_owned(),
            "workflow_dispatch".to_owned(),
        ),
        ("GITHUB_REF".to_owned(), "refs/heads/master".to_owned()),
        (
            "GITHUB_STEP_SUMMARY".to_owned(),
            fixture
                .project
                .temporary_directory()
                .join("summary")
                .to_str()
                .unwrap()
                .to_owned(),
        ),
    ]
    .into_iter()
    .collect();
    fixture.engine.components.execution = Arc::new(
        crate::execution::GitHubExecutionProvider::from_values(values, "master".into()),
    );
    assert!(
        fixture
            .engine
            .release(ReleaseRequest::default())
            .unwrap_err()
            .to_string()
            .contains("durable artifact uploader")
    );
    assert_eq!(fixture.publisher.publication_count(), 0);
    assert_eq!(fixture.store.read()?.unwrap().phase, Phase::Preserve);
    Ok(())
}

struct UntrustedHostEdit(PathBuf);
impl crate::hooks::LifecycleHook<Pushed, Hosted> for UntrustedHostEdit {
    fn id(&self) -> &str {
        "unexpected-host-edit"
    }
    fn after(&self, _: &Hosted, _: &ExecutionContext) -> Result {
        fs::write(self.0.join("README.md"), "Unexpected host edit.\n")?;
        Ok(())
    }
}

#[test]
fn next_preparation_never_stages_unexpected_edits_from_an_earlier_phase() -> Result {
    let mut fixture = Fixture::new()?;
    fixture.engine.hooks.host = fixture
        .engine
        .hooks
        .host
        .with(UntrustedHostEdit(fixture.project.root().into()));
    assert!(
        fixture
            .engine
            .release(ReleaseRequest::default())
            .unwrap_err()
            .to_string()
            .contains("clean checkout")
    );
    assert_eq!(fixture.publisher.publication_count(), 1);
    assert_eq!(fixture.package.metadata()?.version, Version::new(0, 1, 0));
    assert!(
        command::text(command::run(
            fixture.project.root(),
            "git",
            &["diff", "--cached", "--name-only"]
        )?)?
        .is_empty()
    );
    assert_eq!(
        fs::read_to_string(fixture.project.root().join("README.md"))?,
        "Unexpected host edit.\n"
    );
    Ok(())
}
