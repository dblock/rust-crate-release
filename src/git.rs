use crate::command;
use crate::semantic::{SemanticCommit, SemanticHistory, SemanticHistoryData, SemanticTag};
use crate::{Error, Result};
use semver::Version;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct RepositoryState {
    pub head: String,
    pub tree: String,
    pub branch: String,
    pub remote_head: String,
    pub timestamp: i64,
    pub shallow: bool,
}

pub trait ReleaseRepository: Send + Sync {
    fn root(&self) -> &Path;
    fn inspect(&self) -> Result<RepositoryState>;
    fn timestamp(&self, commit: &str) -> Result<i64>;
    fn contains(&self, ancestor: &str, descendant: &str) -> Result<bool>;
    fn assert_clean(&self) -> Result;
    fn stage_changes(&self) -> Result<String>;
    fn assert_prepared(&self, tree: &str) -> Result;
    fn commit(&self, tree: &str, message: &str) -> Result<String>;
    fn ensure_tag(&self, tag: &str, commit: &str) -> Result;
    fn local_tag(&self, tag: &str) -> Result<Option<String>>;
    fn remote_tag(&self, tag: &str) -> Result<Option<String>>;
    fn preserve_bundle(&self, path: &Path, tag: &str, commit: &str) -> Result;
    fn push_release(&self, commit: &str, tag: &str, expected_remote: &str) -> Result;
    fn push_development(&self, commit: &str, expected_remote: &str) -> Result;
    fn artifact_directory(&self) -> Result<PathBuf>;
}

#[derive(Clone, Debug)]
pub struct Git {
    root: PathBuf,
    branch: String,
    remote: String,
}

impl Git {
    pub fn new(
        root: impl Into<PathBuf>,
        branch: impl Into<String>,
        remote: impl Into<String>,
    ) -> Result<Self> {
        let root = dunce::canonicalize(root.into())?;
        let branch = branch.into();
        let remote = remote.into();
        if remote.is_empty()
            || remote.starts_with('-')
            || !remote
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
        {
            return Err(Error::invalid("invalid Git remote name"));
        }
        command::run(&root, "git", &["check-ref-format", "--branch", &branch])?;
        let actual = command::text(command::run(
            &root,
            "git",
            &["rev-parse", "--show-toplevel"],
        )?)?;
        if dunce::canonicalize(Path::new(&actual))? != root {
            return Err(Error::invalid("release directory must be the Git root"));
        }
        Ok(Self {
            root,
            branch,
            remote,
        })
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        command::text(command::run(&self.root, "git", args)?)
    }

    fn object_id(value: &str) -> Result {
        if !matches!(value.len(), 40 | 64) || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::invalid("expected a complete Git object ID"));
        }
        Ok(())
    }

    fn check_tag(&self, tag: &str) -> Result {
        command::run(
            &self.root,
            "git",
            &["check-ref-format", &format!("refs/tags/{tag}")],
        )?;
        Ok(())
    }

    fn remote_ref(&self, reference: &str) -> Result<Option<String>> {
        let output = self.git(&["ls-remote", "--refs", &self.remote, reference])?;
        if output.is_empty() {
            return Ok(None);
        }
        let mut lines = output.lines();
        let line = lines.next().unwrap();
        if lines.next().is_some() {
            return Err(Error::invalid("ambiguous remote reference"));
        }
        let (sha, found) = line
            .split_once('\t')
            .ok_or_else(|| Error::invalid("malformed remote reference"))?;
        Self::object_id(sha)?;
        if found != reference {
            return Err(Error::invalid("remote returned an unexpected reference"));
        }
        Ok(Some(sha.into()))
    }

    pub fn remote_head(&self) -> Result<String> {
        self.remote_ref(&format!("refs/heads/{}", self.branch))?
            .ok_or_else(|| Error::invalid("configured remote branch does not exist"))
    }

    pub fn timestamp(&self, commit: &str) -> Result<i64> {
        Self::object_id(commit)?;
        self.git(&["show", "-s", "--format=%ct", commit])?
            .parse()
            .map_err(|_| Error::invalid("invalid Git source timestamp"))
    }

    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        Self::object_id(ancestor)?;
        Self::object_id(descendant)?;
        let output = std::process::Command::new("git")
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .current_dir(&self.root)
            .output()?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(Error::invalid("could not establish Git ancestry")),
        }
    }

    fn remote_contains(&self, commit: &str, remote_head: &str) -> Result<bool> {
        if commit == remote_head {
            return Ok(true);
        }
        Self::object_id(remote_head)?;
        self.git(&["fetch", "--no-tags", &self.remote, remote_head])?;
        self.is_ancestor(commit, remote_head)
    }

    fn changed_paths(&self) -> Result<Vec<String>> {
        let mut paths = BTreeSet::new();
        for args in [
            vec!["diff", "--name-only", "-z", "HEAD", "--"],
            vec!["ls-files", "--others", "--exclude-standard", "-z"],
        ] {
            let output = command::run(&self.root, "git", &args)?;
            let source = String::from_utf8(output.stdout)
                .map_err(|_| Error::invalid("release paths must be UTF-8"))?;
            paths.extend(
                source
                    .split('\0')
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned),
            );
        }
        Ok(paths.into_iter().collect())
    }
}

#[derive(Clone, Debug)]
pub struct GitHistory {
    repository: Git,
}

impl GitHistory {
    pub fn new(repository: Git) -> Self {
        Self { repository }
    }
}

impl SemanticHistory for GitHistory {
    fn read(&self, source_sha: &str) -> Result<SemanticHistoryData> {
        Git::object_id(source_sha)?;
        if self
            .repository
            .git(&["rev-parse", "--is-shallow-repository"])?
            != "false"
        {
            return Err(Error::invalid("semantic releases require full Git history"));
        }
        let tags = self.repository.git(&["tag", "--merged", source_sha])?;
        let mut last_release: Option<SemanticTag> = None;
        for tag in tags.lines() {
            let Some(number) = tag.strip_prefix('v') else {
                continue;
            };
            let Ok(version) = Version::parse(number) else {
                continue;
            };
            if !version.pre.is_empty() || !version.build.is_empty() || tag != format!("v{version}")
            {
                continue;
            }
            if last_release
                .as_ref()
                .is_none_or(|last| version > last.version)
            {
                last_release = Some(SemanticTag {
                    tag: tag.into(),
                    version,
                    commit: self
                        .repository
                        .local_tag(tag)?
                        .ok_or_else(|| Error::invalid("release tag disappeared"))?,
                });
            }
        }
        let range = last_release
            .as_ref()
            .map(|last| format!("{}..{source_sha}", last.commit))
            .unwrap_or_else(|| source_sha.into());
        let output = self
            .repository
            .git(&["log", "--reverse", "--format=%H%x00%B%x00", &range])?;
        let mut fields = output.split('\0');
        let mut commits = Vec::new();
        while let Some(sha) = fields.next().map(str::trim).filter(|v| !v.is_empty()) {
            Git::object_id(sha)?;
            let message = fields
                .next()
                .ok_or_else(|| Error::invalid("malformed Git history"))?;
            commits.push(SemanticCommit {
                sha: sha.into(),
                message: message.trim_end().into(),
            });
        }
        Ok(SemanticHistoryData {
            last_release,
            commits,
        })
    }
}

impl ReleaseRepository for Git {
    fn root(&self) -> &Path {
        &self.root
    }

    fn inspect(&self) -> Result<RepositoryState> {
        let branch = self.git(&["symbolic-ref", "--short", "HEAD"])?;
        if branch != self.branch {
            return Err(Error::invalid(
                "release requires the configured default branch",
            ));
        }
        let head = self.git(&["rev-parse", "HEAD"])?;
        Self::object_id(&head)?;
        Ok(RepositoryState {
            tree: self.git(&["rev-parse", "HEAD^{tree}"])?,
            remote_head: self.remote_head()?,
            timestamp: self.timestamp(&head)?,
            shallow: self.git(&["rev-parse", "--is-shallow-repository"])? == "true",
            head,
            branch,
        })
    }

    fn timestamp(&self, commit: &str) -> Result<i64> {
        Git::timestamp(self, commit)
    }

    fn contains(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        match self.is_ancestor(ancestor, descendant) {
            Ok(result) => Ok(result),
            Err(_) => self.remote_contains(ancestor, descendant),
        }
    }

    fn assert_clean(&self) -> Result {
        if !self
            .git(&["status", "--porcelain=v1", "--untracked-files=all"])?
            .is_empty()
        {
            return Err(Error::invalid("release requires a clean checkout"));
        }
        Ok(())
    }

    fn stage_changes(&self) -> Result<String> {
        if !self.git(&["ls-files", "--unmerged"])?.is_empty() {
            return Err(Error::invalid("cannot stage an unresolved merge"));
        }
        let paths = self.changed_paths()?;
        if !paths.is_empty() {
            let mut args = vec!["--literal-pathspecs", "add", "--all", "--"];
            args.extend(paths.iter().map(String::as_str));
            self.git(&args)?;
        }
        self.git(&["write-tree"])
    }

    fn assert_prepared(&self, tree: &str) -> Result {
        Self::object_id(tree)?;
        if self.git(&["write-tree"])? != tree
            || !self.git(&["diff", "--name-only", "--"])?.is_empty()
            || !self
                .git(&["ls-files", "--others", "--exclude-standard"])?
                .is_empty()
        {
            return Err(Error::invalid(
                "validation changed tracked, untracked, or staged release files",
            ));
        }
        Ok(())
    }

    fn commit(&self, tree: &str, message: &str) -> Result<String> {
        self.assert_prepared(tree)?;
        if self.git(&["rev-parse", "HEAD^{tree}"])? != tree {
            let head = self.git(&["rev-parse", "HEAD"])?;
            let remote = self.remote_head()?;
            if self.remote_contains(&head, &remote)?
                && self.git(&["rev-parse", &format!("{remote}^{{tree}}")])? == tree
            {
                self.git(&["update-ref", "HEAD", &remote, &head])?;
            } else {
                self.git(&["commit", "-m", message])?;
            }
        }
        if self.git(&["rev-parse", "HEAD^{tree}"])? != tree {
            return Err(Error::invalid(
                "release commit does not contain the prepared tree",
            ));
        }
        self.assert_clean()?;
        self.git(&["rev-parse", "HEAD"])
    }

    fn ensure_tag(&self, tag: &str, commit: &str) -> Result {
        Self::object_id(commit)?;
        self.check_tag(tag)?;
        match self.local_tag(tag)? {
            Some(existing) if existing != commit => Err(Error::invalid(
                "release tag already points to a different commit",
            )),
            Some(_) => Ok(()),
            None => {
                self.git(&["tag", "--", tag, commit])?;
                Ok(())
            }
        }
    }

    fn local_tag(&self, tag: &str) -> Result<Option<String>> {
        self.check_tag(tag)?;
        if self.git(&["tag", "--list", "--", tag])?.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.git(&[
            "rev-parse",
            "--verify",
            &format!("refs/tags/{tag}^{{commit}}"),
        ])?))
    }

    fn remote_tag(&self, tag: &str) -> Result<Option<String>> {
        self.check_tag(tag)?;
        let reference = format!("refs/tags/{tag}");
        let output = self.git(&[
            "ls-remote",
            &self.remote,
            &reference,
            &format!("{reference}^{{}}"),
        ])?;
        let mut direct = None;
        let mut peeled = None;
        for line in output.lines() {
            let (sha, found) = line
                .split_once('\t')
                .ok_or_else(|| Error::invalid("malformed remote tag"))?;
            Self::object_id(sha)?;
            if found == reference {
                if direct.replace(sha.to_owned()).is_some() {
                    return Err(Error::invalid("duplicate remote tag"));
                }
            } else if found == format!("{reference}^{{}}") {
                if peeled.replace(sha.to_owned()).is_some() {
                    return Err(Error::invalid("duplicate peeled remote tag"));
                }
            } else {
                return Err(Error::invalid("unexpected remote tag reference"));
            }
        }
        Ok(peeled.or(direct))
    }

    fn preserve_bundle(&self, path: &Path, tag: &str, commit: &str) -> Result {
        self.check_tag(tag)?;
        Self::object_id(commit)?;
        let reference = format!("refs/tags/{tag}");
        if self.local_tag(tag)?.as_deref() != Some(commit) {
            return Err(Error::invalid(
                "bundle requires the matching local release tag",
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| Error::invalid("bundle needs a parent"))?;
        fs::create_dir_all(parent)?;
        if path.exists() {
            self.git(&[
                "bundle",
                "verify",
                path.to_str()
                    .ok_or_else(|| Error::invalid("bundle path must be UTF-8"))?,
            ])?;
            let output = self.git(&[
                "bundle",
                "list-heads",
                path.to_str()
                    .ok_or_else(|| Error::invalid("bundle path must be UTF-8"))?,
                &reference,
            ])?;
            if output
                .lines()
                .any(|line| line == format!("{commit} {reference}"))
            {
                return Ok(());
            }
            return Err(Error::invalid(
                "retained recovery bundle conflicts with release",
            ));
        }
        let temporary = tempfile::NamedTempFile::new_in(parent)?;
        self.git(&[
            "bundle",
            "create",
            temporary
                .path()
                .to_str()
                .ok_or_else(|| Error::invalid("bundle path must be UTF-8"))?,
            &reference,
        ])?;
        temporary.persist_noclobber(path).map_err(|e| e.error)?;
        Ok(())
    }

    fn push_release(&self, commit: &str, tag: &str, expected_remote: &str) -> Result {
        Self::object_id(commit)?;
        Self::object_id(expected_remote)?;
        if self.local_tag(tag)?.as_deref() != Some(commit) {
            return Err(Error::invalid(
                "release push requires the matching local tag",
            ));
        }
        let existing_tag = self.remote_tag(tag)?;
        if existing_tag.as_deref().is_some_and(|sha| sha != commit) {
            return Err(Error::invalid("remote release tag conflicts with release"));
        }
        let remote = self.remote_head()?;
        let advance_branch = remote == expected_remote;
        if !advance_branch && !self.remote_contains(commit, &remote)? {
            return Err(Error::invalid("remote branch changed concurrently"));
        }
        let branch_spec = format!("{commit}:refs/heads/{}", self.branch);
        let tag_spec = format!("refs/tags/{tag}:refs/tags/{tag}");
        let mut args = vec!["push", "--atomic", &self.remote];
        if advance_branch {
            args.push(&branch_spec);
        }
        if existing_tag.is_none() {
            args.push(&tag_spec);
        }
        if args.len() > 3 {
            self.git(&args)?;
        }
        if self.remote_tag(tag)?.as_deref() != Some(commit)
            || !self.remote_contains(commit, &self.remote_head()?)?
        {
            return Err(Error::invalid("remote release refs could not be verified"));
        }
        Ok(())
    }

    fn push_development(&self, commit: &str, expected_remote: &str) -> Result {
        Self::object_id(commit)?;
        Self::object_id(expected_remote)?;
        let remote = self.remote_head()?;
        if remote == expected_remote {
            self.git(&[
                "push",
                &self.remote,
                &format!("{commit}:refs/heads/{}", self.branch),
            ])?;
        } else if !self.remote_contains(commit, &remote)? {
            return Err(Error::invalid(
                "remote branch changed before development push",
            ));
        }
        if !self.remote_contains(commit, &self.remote_head()?)? {
            return Err(Error::invalid("next development ref could not be verified"));
        }
        Ok(())
    }

    fn artifact_directory(&self) -> Result<PathBuf> {
        let path = PathBuf::from(self.git(&["rev-parse", "--git-path", "rcr"])?);
        Ok(if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::GitProject;

    fn repository(project: &GitProject) -> Result<Git> {
        Git::new(project.root(), "master", "origin")
    }

    fn edit_commit(git: &Git, name: &str, body: &str, message: &str) -> Result<String> {
        fs::write(git.root().join(name), body)?;
        let tree = git.stage_changes()?;
        git.commit(&tree, message)
    }

    #[test]
    fn trusted_staging_is_literal_and_validation_cannot_change_the_tree() -> Result {
        let project = GitProject::new(&[("README.md", "Initial\n")])?;
        let git = repository(&project)?;
        git.assert_clean()?;
        fs::write(git.root().join("literal[1].md"), "Unicode crab\n")?;
        fs::write(git.root().join("literal1.md"), "Other\n")?;
        let tree = git.stage_changes()?;
        git.assert_prepared(&tree)?;
        fs::write(git.root().join("README.md"), "Unexpected\n")?;
        assert!(git.assert_prepared(&tree).is_err());
        assert!(git.assert_clean().is_err());
        assert!(git.commit(&tree, "Must Fail").is_err());
        assert_eq!(
            fs::read_to_string(git.root().join("README.md"))?,
            "Unexpected\n"
        );
        Ok(())
    }

    #[test]
    fn release_refs_bundle_and_next_push_reconcile_without_rewinding() -> Result {
        let project = GitProject::new(&[("README.md", "Initial\n")])?;
        let git = repository(&project)?;
        let source = git.inspect()?;
        let release = edit_commit(&git, "README.md", "Released\n", "Release 0.1.0")?;
        git.ensure_tag("v0.1.0", &release)?;
        git.ensure_tag("v0.1.0", &release)?;
        assert!(git.ensure_tag("v0.1.0", &source.head).is_err());
        let bundle = git.artifact_directory()?.join("release.bundle");
        git.preserve_bundle(&bundle, "v0.1.0", &release)?;
        let original_bundle = fs::read(&bundle)?;
        git.preserve_bundle(&bundle, "v0.1.0", &release)?;
        assert_eq!(fs::read(bundle)?, original_bundle);
        git.push_release(&release, "v0.1.0", &source.remote_head)?;
        git.push_release(&release, "v0.1.0", &source.remote_head)?;
        assert_eq!(git.remote_tag("v0.1.0")?.as_deref(), Some(release.as_str()));
        let next = edit_commit(&git, "README.md", "Development\n", "Prepare 0.1.1")?;
        git.push_development(&next, &release)?;
        git.push_development(&next, &release)?;
        git.push_release(&release, "v0.1.0", &source.remote_head)?;
        assert_eq!(git.remote_head()?, next);
        assert_eq!(git.commit(&git.inspect()?.tree, "No Duplicate")?, next);
        Ok(())
    }

    #[test]
    fn matching_remote_preparation_is_reused_by_fast_forward_without_rewriting_source() -> Result {
        let project = GitProject::new(&[("README.md", "Release\n")])?;
        let git = repository(&project)?;
        let release = git.inspect()?.head;
        fs::write(git.root().join("README.md"), "Development\n")?;
        let tree = git.stage_changes()?;
        let remote = git.git(&[
            "commit-tree",
            &tree,
            "-p",
            &release,
            "-m",
            "Other Runner Prepared Development",
        ])?;
        git.git(&["push", "origin", &format!("{remote}:refs/heads/master")])?;
        assert_eq!(git.commit(&tree, "Do Not Duplicate")?, remote);
        assert_eq!(git.inspect()?.head, remote);
        assert_eq!(
            fs::read_to_string(git.root().join("README.md"))?,
            "Development\n"
        );
        git.assert_clean()?;
        Ok(())
    }

    #[test]
    fn remote_descendants_are_preserved_and_conflicting_refs_or_inputs_fail_closed() -> Result {
        let project = GitProject::new(&[("README.md", "Initial\n")])?;
        let git = repository(&project)?;
        let initial = git.inspect()?;
        let release = edit_commit(&git, "README.md", "Release\n", "Release")?;
        git.ensure_tag("v0.1.0", &release)?;
        let other = edit_commit(&git, "README.md", "Other\n", "Other")?;
        command::run(
            project.root(),
            "git",
            &["push", "origin", &format!("{other}:refs/heads/master")],
        )?;
        assert!(
            git.push_release(&release, "v0.1.0", &"f".repeat(40))
                .is_ok()
        );
        command::run(project.root(), "git", &["tag", "v2.0.0", &initial.head])?;
        command::run(
            project.root(),
            "git",
            &["push", "origin", "refs/tags/v2.0.0"],
        )?;
        assert!(git.push_release(&release, "v2.0.0", &initial.head).is_err());
        assert!(Git::new(project.root(), "master", "--upload-pack=bad").is_err());
        assert!(git.ensure_tag("bad tag", &release).is_err());
        assert!(git.is_ancestor("HEAD", &other).is_err());
        assert!(Git::new(project.root().join(".git"), "master", "origin").is_err());
        command::run(project.root(), "git", &["switch", "-c", "feature"])?;
        assert!(git.inspect().is_err());
        Ok(())
    }

    #[test]
    fn concurrent_remote_history_is_not_overwritten_or_partially_tagged() -> Result {
        let project = GitProject::new(&[("README.md", "Initial\n")])?;
        let git = repository(&project)?;
        let source = git.inspect()?;
        let release = edit_commit(&git, "README.md", "Released\n", "Release")?;
        git.ensure_tag("v0.1.0", &release)?;
        let other = git.git(&[
            "commit-tree",
            &source.tree,
            "-p",
            &source.head,
            "-m",
            "Concurrent Change",
        ])?;
        git.git(&["push", "origin", &format!("{other}:refs/heads/master")])?;
        assert!(git.push_release(&release, "v0.1.0", &source.head).is_err());
        assert!(git.push_development(&release, &source.head).is_err());
        assert_eq!(git.remote_head()?, other);
        assert_eq!(git.remote_tag("v0.1.0")?, None);
        assert_eq!(git.inspect()?.head, release);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn failed_atomic_push_preserves_local_release_and_leaves_remote_refs_unchanged() -> Result {
        use std::os::unix::fs::PermissionsExt;
        let project = GitProject::new(&[("README.md", "Initial\n")])?;
        let git = repository(&project)?;
        let source = git.inspect()?;
        let release = edit_commit(&git, "README.md", "Release\n", "Release")?;
        git.ensure_tag("v0.1.0", &release)?;
        let hooks = project.temporary_directory().join("reject-hooks");
        fs::create_dir(&hooks)?;
        fs::write(hooks.join("pre-receive"), "#!/bin/sh\nexit 1\n")?;
        fs::set_permissions(hooks.join("pre-receive"), fs::Permissions::from_mode(0o755))?;
        command::run(
            project.remote(),
            "git",
            &[
                "--git-dir",
                project.remote().to_str().unwrap(),
                "config",
                "core.hooksPath",
                hooks.to_str().unwrap(),
            ],
        )?;
        assert!(git.push_release(&release, "v0.1.0", &source.head).is_err());
        assert_eq!(git.remote_head()?, source.head);
        assert_eq!(git.remote_tag("v0.1.0")?, None);
        assert_eq!(git.local_tag("v0.1.0")?.as_deref(), Some(release.as_str()));
        assert_eq!(git.inspect()?.head, release);
        Ok(())
    }

    #[test]
    fn history_uses_highest_reachable_numeric_tag_and_preserves_commit_bodies() -> Result {
        let project = GitProject::new(&[("README.md", "Initial\n")])?;
        let git = repository(&project)?;
        let first = git.inspect()?.head;
        git.ensure_tag("v0.9.0", &first)?;
        let second = edit_commit(&git, "README.md", "Second\n", "feat: second\n\nDetails.")?;
        git.ensure_tag("v0.10.0", &second)?;
        git.ensure_tag("v99.0.0-beta.1", &second)?;
        let source = edit_commit(
            &git,
            "README.md",
            "Third\n",
            "fix: third\n\nBody with Unicode: crab.",
        )?;
        let history = GitHistory::new(git).read(&source)?;
        assert_eq!(
            history.last_release.unwrap().version,
            Version::new(0, 10, 0)
        );
        assert_eq!(history.commits.len(), 1);
        assert_eq!(history.commits[0].sha, source);
        assert!(history.commits[0].message.contains("\n\nBody"));
        Ok(())
    }

    #[test]
    fn shallow_semantic_history_fails_before_analysis() -> Result {
        let project = GitProject::new(&[("README.md", "Initial\n")])?;
        let clone = project.temporary_directory().join("shallow");
        let url = reqwest::Url::from_file_path(project.remote())
            .unwrap()
            .to_string();
        command::run(
            project.temporary_directory(),
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
        let git = Git::new(&clone, "master", "origin")?;
        let source = git.inspect()?.head;
        assert!(GitHistory::new(git).read(&source).is_err());
        Ok(())
    }
}
