use crate::command;
use crate::execution::ExecutionProvider;
use crate::hosting::ReleaseHost;
use crate::package::Artifact;
use crate::registry::{PackagePublisher, PublishOutcome, PublishedCrate};
use crate::{Error, Release, Result};
use semver::Version;
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Debug)]
pub enum CopySelection {
    All,
    Paths(Vec<PathBuf>),
    Glob(String),
}

#[derive(Clone, Debug)]
pub struct DirectoryReleaseHost {
    root: PathBuf,
    publications: Arc<AtomicUsize>,
}

impl DirectoryReleaseHost {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self {
            root: root.canonicalize()?,
            publications: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub fn publication_count(&self) -> usize {
        self.publications.load(Ordering::SeqCst)
    }
}

fn atomic_write(root: &Path, path: &Path, bytes: &[u8]) -> Result {
    let mut temporary = tempfile::NamedTempFile::new_in(root)?;
    temporary.write_all(bytes)?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

impl ReleaseHost for DirectoryReleaseHost {
    fn publish(&self, release: &Release, commit: &str, notes: &str, artifact: &Artifact) -> Result {
        if release.name() != artifact.name()
            || release.version() != artifact.version()
            || !matches!(commit.len(), 40 | 64)
            || !commit.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(Error::invalid(
                "local hosting inputs do not match the captured release",
            ));
        }
        let bytes = artifact.validated_bytes()?;
        let directory = self.root.join(release.tag());
        if fs::symlink_metadata(&directory).is_ok_and(|metadata| !metadata.is_dir()) {
            return Err(Error::invalid(
                "local hosted release directory must not be a link",
            ));
        }
        fs::create_dir_all(&directory)?;
        let metadata_path = directory.join("release.json");
        let archive_path =
            directory.join(format!("{}-{}.crate", release.name(), release.version()));
        for path in [&metadata_path, &archive_path] {
            if fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.is_file()) {
                return Err(Error::invalid("local hosted outputs must be regular files"));
            }
        }
        let metadata = serde_json::to_vec_pretty(&serde_json::json!({
            "tag": release.tag(),
            "commit": commit,
            "notes": notes,
            "name": release.name(),
            "version": release.version(),
            "checksum": artifact.checksum(),
        }))?;
        if fs::read(&metadata_path).ok().as_deref() == Some(metadata.as_slice())
            && fs::read(&archive_path).ok().as_deref() == Some(bytes.as_slice())
        {
            return Ok(());
        }
        atomic_write(&directory, &archive_path, &bytes)?;
        atomic_write(&directory, &metadata_path, &metadata)?;
        self.publications.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// An explicit local publication adapter, never selected by production detection.
#[derive(Clone, Debug)]
pub struct LocalRegistry {
    root: PathBuf,
    publications: Arc<AtomicUsize>,
}

impl LocalRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self {
            root: root.canonicalize()?,
            publications: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub fn publication_count(&self) -> usize {
        self.publications.load(Ordering::SeqCst)
    }

    fn archive_path(&self, name: &str, version: &Version) -> Result<PathBuf> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            || !version.pre.is_empty()
            || !version.build.is_empty()
        {
            return Err(Error::invalid("invalid local registry package identity"));
        }
        Ok(self.root.join(format!("{name}-{version}.crate")))
    }
}

impl PackagePublisher for LocalRegistry {
    fn lookup(&self, name: &str, version: &Version) -> Result<Option<PublishedCrate>> {
        let path = self.archive_path(name, version)?;
        if !path.exists() {
            return Ok(None);
        }
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(Error::invalid(
                "local registry archives must not be symbolic links",
            ));
        }
        let artifact = Artifact::open(&path)?;
        if artifact.name() != name || artifact.version() != version {
            return Err(Error::invalid(
                "local registry archive identity conflicts with lookup",
            ));
        }
        Ok(Some(PublishedCrate {
            name: name.into(),
            version: version.clone(),
            checksum: artifact.checksum().into(),
        }))
    }

    fn publish(
        &self,
        artifact: &Artifact,
        _provider: &dyn ExecutionProvider,
    ) -> Result<PublishOutcome> {
        let bytes = artifact.validated_bytes()?;
        if let Some(existing) = self.lookup(artifact.name(), artifact.version())? {
            if existing.checksum != artifact.checksum() {
                return Err(Error::invalid(
                    "local registry version already has different bytes",
                ));
            }
            return Ok(PublishOutcome::Reused);
        }
        let destination = self.archive_path(artifact.name(), artifact.version())?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        temporary.write_all(&bytes)?;
        temporary
            .persist_noclobber(destination)
            .map_err(|error| error.error)?;
        self.publications.fetch_add(1, Ordering::SeqCst);
        Ok(PublishOutcome::Published)
    }

    fn download(&self, name: &str, version: &Version) -> Result<Vec<u8>> {
        self.lookup(name, version)?
            .ok_or_else(|| Error::invalid("local registry version is missing"))?;
        Ok(fs::read(self.archive_path(name, version)?)?)
    }
}

/// Disposable real Git repositories; publication adapters remain explicit.
pub struct GitProject {
    temporary: tempfile::TempDir,
    root: PathBuf,
    remote: PathBuf,
}

impl GitProject {
    pub fn new(files: &[(&str, &str)]) -> Result<Self> {
        Self::create(files, None)
    }

    pub fn from_project(source: &Path, selection: &CopySelection) -> Result<Self> {
        Self::create(&[], Some((source, selection)))
    }

    fn create(files: &[(&str, &str)], source: Option<(&Path, &CopySelection)>) -> Result<Self> {
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().join("project");
        let remote = temporary.path().join("remote.git");
        fs::create_dir_all(&root)?;
        if let Some((source, selection)) = source {
            copy_project_selected(source, &root, selection)?;
        }
        command::run(
            temporary.path(),
            "git",
            &["init", "--bare", "-b", "master", path_text(&remote)?],
        )?;
        command::run(&root, "git", &["init", "-b", "master"])?;
        command::run(&root, "git", &["config", "user.name", "Release Fixture"])?;
        command::run(
            &root,
            "git",
            &["config", "user.email", "fixture@example.invalid"],
        )?;
        command::run(&root, "git", &["config", "commit.gpgsign", "false"])?;
        command::run(&root, "git", &["config", "tag.gpgsign", "false"])?;
        command::run(&root, "git", &["config", "core.autocrlf", "false"])?;
        let hooks = temporary.path().join("disabled-hooks");
        fs::create_dir(&hooks)?;
        command::run(
            &root,
            "git",
            &["config", "core.hooksPath", path_text(&hooks)?],
        )?;
        command::run(
            &remote,
            "git",
            &[
                "--git-dir",
                path_text(&remote)?,
                "config",
                "core.hooksPath",
                path_text(&hooks)?,
            ],
        )?;
        if !root.join(".gitignore").exists() {
            fs::write(root.join(".gitignore"), "/target/\n/.rcr/\n")?;
        }
        for (path, contents) in files {
            safe_relative(Path::new(path))?;
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap())?;
            fs::write(path, contents)?;
        }
        command::run(&root, "git", &["add", "--all"])?;
        command::run(&root, "git", &["commit", "-m", "Initialize Fixture"])?;
        command::run(
            &root,
            "git",
            &["remote", "add", "origin", path_text(&remote)?],
        )?;
        command::run(&root, "git", &["push", "-u", "origin", "master"])?;
        Ok(Self {
            temporary,
            root,
            remote,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn remote(&self) -> &Path {
        &self.remote
    }
    pub fn temporary_directory(&self) -> &Path {
        self.temporary.path()
    }

    pub fn cargo_library() -> Result<Self> {
        Self::new(&[
            (
                "Cargo.toml",
                "[package]\nname = \"release-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\nlicense = \"MIT\"\ndescription = \"Isolated release fixture\"\n",
            ),
            ("src/lib.rs", "pub fn answer() -> u8 {\n    42\n}\n"),
            (
                "Cargo.lock",
                "version = 4\n\n[[package]]\nname = \"release-fixture\"\nversion = \"0.1.0\"\n",
            ),
            (
                "CHANGELOG.md",
                "# Changelog\n\n## 0.1.0 (Next)\n\n- Initial fixture.\n- Your contribution here.\n",
            ),
        ])
    }
}

type ConfigureFixture = dyn Fn(&mut crate::engine::ReleaseWorkflow) -> Result + Send + Sync;

/// A configured production CLI/engine with real disposable Git and local destinations.
pub struct ReleaseFixture {
    pub project: GitProject,
    pub registry: Arc<LocalRegistry>,
    pub host: Arc<DirectoryReleaseHost>,
    configure: Arc<ConfigureFixture>,
}

impl ReleaseFixture {
    pub fn new(project: GitProject) -> Result<Self> {
        let config = crate::cli::Configuration::load(project.root(), None)?;
        if config.remote != "origin" {
            command::run(
                project.root(),
                "git",
                &["remote", "rename", "origin", &config.remote],
            )?;
        }
        if !config.default_branch.is_empty() && config.default_branch != "master" {
            command::run(
                project.root(),
                "git",
                &["branch", "-m", &config.default_branch],
            )?;
            command::run(
                project.root(),
                "git",
                &["push", "-u", &config.remote, &config.default_branch],
            )?;
            command::run(
                project.root(),
                "git",
                &[
                    "--git-dir",
                    path_text(project.remote())?,
                    "symbolic-ref",
                    "HEAD",
                    &format!("refs/heads/{}", config.default_branch),
                ],
            )?;
        }
        Ok(Self {
            registry: Arc::new(LocalRegistry::new(
                project.temporary_directory().join("registry"),
            )?),
            host: Arc::new(DirectoryReleaseHost::new(
                project.temporary_directory().join("host"),
            )?),
            project,
            configure: Arc::new(|_| Ok(())),
        })
    }

    pub fn configure_workflow(
        &mut self,
        configure: impl Fn(&mut crate::engine::ReleaseWorkflow) -> Result + Send + Sync + 'static,
    ) -> &mut Self {
        self.configure = Arc::new(configure);
        self
    }

    pub fn release(&self, version: Option<&str>) -> Result<serde_json::Value> {
        let mut args = vec!["release"];
        if let Some(version) = version {
            args.extend(["--release-version", version]);
        }
        self.run(&args)
    }

    pub fn run(&self, args: &[&str]) -> Result<serde_json::Value> {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from(self.arguments(args))
            .map_err(|error| Error::invalid(error.to_string()))?;
        crate::cli::run_with(cli, &|workflow| {
            (self.configure)(workflow)?;
            workflow.components.publisher = self.registry.clone();
            workflow.components.host = Some(self.host.clone());
            workflow.components.execution = Arc::new(crate::execution::LocalExecutionProvider);
            workflow.components.preserver = Arc::new(crate::engine::LocalArtifacts);
            workflow.components.verifier = Arc::new(crate::engine::RegistryVerifier::new(
                1,
                std::time::Duration::ZERO,
            )?);
            Ok(())
        })
    }

    fn arguments(&self, args: &[&str]) -> Vec<std::ffi::OsString> {
        let mut command = vec![
            "rcr".into(),
            "--root".into(),
            self.project.root().as_os_str().to_owned(),
            "--test-registry".into(),
            self.registry.root.as_os_str().to_owned(),
            "--test-host".into(),
            self.host.root.as_os_str().to_owned(),
        ];
        command.extend(args.iter().map(|arg| (*arg).into()));
        command
    }

    /// Executes a compiled project runner or rcr with explicit local destinations.
    pub fn process(
        &self,
        program: &Path,
        prefix: &[&str],
        args: &[&str],
    ) -> Result<serde_json::Value> {
        let mut command = std::process::Command::new(program);
        command
            .args(prefix)
            .args(self.arguments(args).into_iter().skip(1))
            .current_dir(self.project.root())
            .env("RCR_FIXTURE_ACTIVE", "1")
            .env_remove("GITHUB_ACTIONS")
            .env_remove("CARGO_REGISTRY_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_TOKEN")
            .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
            .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
            .env_remove("RCR_RUNNER")
            .env_remove("RCR_CONFIG")
            .env_remove("RCR_RUNNER_ACTIVE");
        for key in [
            "RCR_PREPARE_COMMAND",
            "RCR_PUBLISH_COMMAND",
            "RCR_BUMP_COMMAND",
            "RCR_VALIDATE_COMMAND",
            "RCR_TEST_COMMAND",
            "RCR_SUCCESS_COMMAND",
            "RCR_FAILURE_COMMAND",
        ] {
            command.env_remove(key);
        }
        let output = command.output()?;
        if !output.status.success() {
            return Err(Error::invalid(format!(
                "fixture process failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|_| Error::invalid("fixture runner must print one JSON result"))
    }

    pub fn remote_file(&self, reference: &str, path: &Path) -> Result<Vec<u8>> {
        safe_relative(path)?;
        let commit = command::text(command::run(
            self.project.root(),
            "git",
            &[
                "--git-dir",
                path_text(self.project.remote())?,
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{reference}^{{commit}}"),
            ],
        )?)?;
        Ok(command::run(
            self.project.root(),
            "git",
            &[
                "--git-dir",
                path_text(self.project.remote())?,
                "show",
                &format!("{commit}:{}", path_text(path)?),
            ],
        )?
        .stdout)
    }
}

/// Guard a release integration test when its child validator runs Cargo tests.
pub fn in_release_fixture() -> bool {
    std::env::var("RCR_FIXTURE_ACTIVE").as_deref() == Ok("1")
}

pub(crate) fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| Error::invalid("fixture path must be UTF-8"))
}

fn safe_relative(path: &Path) -> Result {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(value) if !git_metadata(value)))
    {
        return Err(Error::invalid(
            "fixture files must be relative and outside Git metadata",
        ));
    }
    Ok(())
}

fn git_metadata(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_lowercase();
    let name = name.trim_end_matches(['.', ' ']);
    name == ".git" || name.starts_with(".git:") || name.starts_with("git~")
}

/// Copy a trusted fixture tree, including hidden files but never Git metadata.
pub fn copy_project(source: &Path, destination: &Path) -> Result {
    copy_project_selected(source, destination, &CopySelection::All)
}

pub fn copy_project_selected(
    source: &Path,
    destination: &Path,
    selection: &CopySelection,
) -> Result {
    let source = source.canonicalize()?;
    if !source.is_dir() {
        return Err(Error::invalid("fixture source must be a directory"));
    }
    if fs::symlink_metadata(destination).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(Error::invalid(
            "fixture destination must not be a symbolic link",
        ));
    }
    let destination = if destination.exists() {
        destination.canonicalize()?
    } else {
        let parent = destination
            .parent()
            .ok_or_else(|| Error::invalid("fixture destination needs a parent"))?
            .canonicalize()?;
        parent.join(
            destination
                .file_name()
                .ok_or_else(|| Error::invalid("fixture destination needs a name"))?,
        )
    };
    if destination.starts_with(&source) || source.starts_with(&destination) {
        return Err(Error::invalid(
            "fixture source and destination must not overlap",
        ));
    }
    let mut directories = Vec::new();
    let paths = match selection {
        CopySelection::Paths(paths) => {
            for path in paths {
                safe_relative(path)?;
                let mut selected = source.clone();
                for component in path.components() {
                    selected.push(component);
                    if fs::symlink_metadata(&selected)?.file_type().is_symlink() {
                        return Err(Error::invalid(
                            "fixture selection must not follow symbolic links",
                        ));
                    }
                }
                if !selected.is_file() || !selected.canonicalize()?.starts_with(&source) {
                    return Err(Error::invalid(
                        "fixture selection must contain files inside its source",
                    ));
                }
            }
            paths.iter().cloned().collect::<BTreeSet<_>>()
        }
        CopySelection::All | CopySelection::Glob(_) => {
            let mut paths = BTreeSet::new();
            collect_files(&source, Path::new(""), &mut paths, &mut directories)?;
            if let CopySelection::Glob(pattern) = selection {
                if pattern.contains('\\') {
                    return Err(Error::invalid("fixture globs use portable forward slashes"));
                }
                let matcher = globset::GlobBuilder::new(pattern)
                    .literal_separator(true)
                    .backslash_escape(false)
                    .build()
                    .map_err(|error| Error::invalid(format!("invalid fixture glob: {error}")))?
                    .compile_matcher();
                paths.retain(|path| matcher.is_match(path));
                if paths.is_empty() {
                    return Err(Error::invalid("fixture glob did not select any files"));
                }
            }
            paths
        }
    };
    fs::create_dir_all(&destination)?;
    if matches!(selection, CopySelection::All) {
        for directory in directories {
            create_destination_directory(&destination, &directory)?;
        }
    }
    for relative in paths {
        let target = destination.join(&relative);
        if let Some(relative_parent) = relative.parent() {
            create_destination_directory(&destination, relative_parent)?;
        }
        if fs::symlink_metadata(&target).is_ok() {
            return Err(Error::invalid(
                "fixture copying must not overwrite existing files",
            ));
        }
        fs::copy(source.join(relative), target)?;
    }
    Ok(())
}

fn create_destination_directory(base: &Path, relative: &Path) -> Result {
    let mut path = base.to_owned();
    for component in relative.components() {
        path.push(component);
        if fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
        {
            return Err(Error::invalid(
                "fixture destination contains a link or non-directory",
            ));
        }
        fs::create_dir_all(&path)?;
    }
    Ok(())
}

fn collect_files(
    source: &Path,
    relative: &Path,
    paths: &mut BTreeSet<PathBuf>,
    directories: &mut Vec<PathBuf>,
) -> Result {
    for entry in fs::read_dir(source.join(relative))? {
        let entry = entry?;
        if git_metadata(&entry.file_name()) {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(Error::invalid(
                "fixture copying does not follow symbolic links",
            ));
        }
        let target = relative.join(entry.file_name());
        if kind.is_dir() {
            directories.push(target.clone());
            collect_files(source, &target, paths, directories)?;
        } else if kind.is_file() {
            safe_relative(&target)?;
            paths.insert(target);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::LocalExecutionProvider;
    use crate::package::{CargoPackage, PackageProject};
    use crate::store::{DirectoryReleaseStore, ReleaseReceipt, ReleaseStore};

    #[test]
    fn local_registry_publishes_exact_bytes_once_and_refuses_version_conflicts() -> Result {
        let project = GitProject::cargo_library()?;
        let package = CargoPackage::new(project.root())?;
        let first = package.build(&project.temporary_directory().join("first"), 1_700_000_000)?;
        let registry = LocalRegistry::new(project.temporary_directory().join("registry"))?;
        assert!(registry.lookup(first.name(), first.version())?.is_none());
        assert_eq!(
            registry.publish(&first, &LocalExecutionProvider)?,
            PublishOutcome::Published
        );
        assert_eq!(
            registry.publish(&first, &LocalExecutionProvider)?,
            PublishOutcome::Reused
        );
        assert_eq!(registry.publication_count(), 1);
        assert_eq!(
            registry.download(first.name(), first.version())?,
            fs::read(first.path())?
        );
        fs::write(
            project.root().join("src/lib.rs"),
            "pub fn answer() -> u8 { 43 }\n",
        )?;
        let second = package.build(&project.temporary_directory().join("second"), 1_700_000_000)?;
        assert_ne!(first.checksum(), second.checksum());
        assert!(registry.publish(&second, &LocalExecutionProvider).is_err());
        assert_eq!(registry.publication_count(), 1);
        assert_eq!(
            registry.download(first.name(), first.version())?,
            fs::read(first.path())?
        );
        assert!(registry.lookup("../escape", first.version()).is_err());
        let release = Release::new(
            first.name(),
            first.version().clone(),
            first.version().clone(),
            "a".repeat(40),
            "2026/11/12",
        )?;
        let host = DirectoryReleaseHost::new(project.temporary_directory().join("hosting"))?;
        host.publish(&release, release.source_commit(), "- Initial.", &first)?;
        host.publish(&release, release.source_commit(), "- Initial.", &first)?;
        assert_eq!(host.publication_count(), 1);
        host.publish(&release, release.source_commit(), "- Updated.", &first)?;
        assert_eq!(host.publication_count(), 2);
        let hosted: serde_json::Value = serde_json::from_slice(&fs::read(
            host.root.join(release.tag()).join("release.json"),
        )?)?;
        assert_eq!(hosted["notes"], "- Updated.");
        let store = DirectoryReleaseStore::new(first.path().parent().unwrap())?;
        let mut receipt = ReleaseReceipt::capture(&release, release.source_commit(), 1_700_000_000);
        receipt.record_artifact(&first)?;
        store.write(&receipt)?;
        package.set_version(&Version::new(0, 1, 1))?;
        assert_eq!(
            store.retained_artifact(&store.read()?.unwrap())?.checksum(),
            first.checksum()
        );
        let empty_registry =
            LocalRegistry::new(project.temporary_directory().join("empty-registry"))?;
        let empty_host =
            DirectoryReleaseHost::new(project.temporary_directory().join("empty-hosting"))?;
        fs::write(first.path(), b"changed retained archive")?;
        for registry in [&registry, &empty_registry] {
            assert!(registry.publish(&first, &LocalExecutionProvider).is_err());
        }
        for host in [&host, &empty_host] {
            assert!(
                host.publish(&release, release.source_commit(), "- Tampered.", &first)
                    .is_err()
            );
        }
        assert_eq!(registry.publication_count(), 1);
        assert_eq!(host.publication_count(), 2);
        assert_eq!(fs::read_dir(&empty_registry.root)?.count(), 0);
        assert_eq!(fs::read_dir(&empty_host.root)?.count(), 0);
        assert_eq!(
            fs::read(host.root.join(release.tag()).join("release.json"))?,
            serde_json::to_vec_pretty(&hosted)?
        );
        Ok(())
    }

    #[test]
    fn copies_hidden_files_without_git_metadata_and_preserves_user_files() -> Result {
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("source");
        fs::create_dir_all(source.join(".github"))?;
        fs::create_dir_all(source.join("empty"))?;
        fs::create_dir_all(source.join(".git"))?;
        fs::write(source.join(".github/settings.yml"), "value")?;
        fs::write(source.join(".git/config"), "metadata")?;
        fs::write(source.join("README.md"), "fixture")?;
        let destination = temporary.path().join("consumer");
        copy_project(&source, &destination)?;
        assert_eq!(
            fs::read_to_string(destination.join(".github/settings.yml"))?,
            "value"
        );
        assert!(!destination.join(".git").exists());
        assert!(destination.join("empty").is_dir());
        assert!(copy_project(&source, &source.join("nested")).is_err());
        assert!(copy_project(&source, &destination).is_err());
        assert_eq!(
            fs::read_to_string(destination.join("README.md"))?,
            "fixture"
        );
        for path in [
            "../escape",
            "/absolute",
            ".git/config",
            ".Git/hooks/x",
            ".git./config",
            "git~1/config",
        ] {
            assert!(safe_relative(Path::new(path)).is_err(), "{path}");
        }
        Ok(())
    }

    #[test]
    fn glob_and_exact_array_selection_have_distinct_literal_semantics() -> Result {
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("source");
        fs::create_dir_all(source.join("nested"))?;
        fs::write(source.join("literal[1].txt"), "literal")?;
        fs::write(source.join("literal1.txt"), "matched")?;
        fs::write(source.join(".hidden.txt"), "hidden")?;
        fs::write(source.join("nested/file.txt"), "nested")?;
        let exact = temporary.path().join("exact");
        copy_project_selected(
            &source,
            &exact,
            &CopySelection::Paths(vec!["literal[1].txt".into()]),
        )?;
        assert!(exact.join("literal[1].txt").exists());
        assert!(!exact.join("literal1.txt").exists());
        let glob = temporary.path().join("glob");
        copy_project_selected(&source, &glob, &CopySelection::Glob("**/*.txt".into()))?;
        assert!(glob.join(".hidden.txt").exists());
        assert!(glob.join("nested/file.txt").exists());
        assert!(
            copy_project_selected(
                &source,
                &temporary.path().join("none"),
                &CopySelection::Glob("*.rs".into())
            )
            .is_err()
        );
        assert!(
            copy_project_selected(
                &source,
                &temporary.path().join("escape"),
                &CopySelection::Paths(vec!["../outside".into()])
            )
            .is_err()
        );
        assert!(
            copy_project_selected(
                &source,
                &temporary.path().join("invalid"),
                &CopySelection::Glob("[".into())
            )
            .is_err()
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn copying_refuses_source_or_destination_links() -> Result {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("source");
        let outside = temporary.path().join("outside");
        fs::create_dir(&source)?;
        fs::create_dir(&outside)?;
        fs::write(outside.join("secret"), "outside")?;
        symlink(&outside, source.join("linked"))?;
        assert!(copy_project(&source, &temporary.path().join("consumer")).is_err());
        fs::remove_file(source.join("linked"))?;
        fs::write(source.join("file"), "fixture")?;
        let destination = temporary.path().join("destination");
        fs::create_dir(&destination)?;
        symlink(outside.join("secret"), destination.join("file"))?;
        assert!(copy_project(&source, &destination).is_err());
        assert_eq!(fs::read_to_string(outside.join("secret"))?, "outside");
        Ok(())
    }
}
