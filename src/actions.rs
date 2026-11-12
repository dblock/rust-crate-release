//! Rust Actions orchestration and independent verification of durable GitHub artifacts.

use crate::cli::{Cli, Command, ReleaseArgs};
use crate::engine::ArtifactPreserver;
use crate::execution::ExecutionProvider;
use crate::git::{Git, ReleaseRepository};
use crate::package::{Artifact, checksum};
use crate::{Error, Release, Result};
use clap::Subcommand;
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue, LOCATION};
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::time::Duration;

const ZIP_LIMIT: u64 = 1024 * 1024 * 1024;
const JSON_LIMIT: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Subcommand)]
pub enum ActionCommand {
    /// Validate the caller and locate retained files.
    Guard,
    /// Prepare without publication and expose exact upload paths.
    Prepare,
    /// Verify the uploaded artifact and complete publication.
    Release,
}

fn environment(key: &str) -> Result<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::invalid(format!("Actions requires {key}")))
}

fn guard(values: &impl Fn(&str) -> Option<String>) -> Result {
    let value = |key| values(key).unwrap_or_default();
    let branch = value("RCR_DEFAULT_BRANCH");
    if value("GITHUB_ACTIONS") != "true"
        || value("GITHUB_SERVER_URL") != "https://github.com"
        || value("GITHUB_EVENT_NAME") != "workflow_dispatch"
        || branch.is_empty()
        || value("GITHUB_REF") != format!("refs/heads/{branch}")
        || value("RUNNER_ENVIRONMENT") != "github-hosted"
        || !value("CARGO_REGISTRY_TOKEN").is_empty()
    {
        return Err(Error::invalid(
            "releases require manual default-branch dispatch on github.com hosted runners with OIDC",
        ));
    }
    Ok(())
}

fn output(name: &str, value: &str) -> Result {
    if value.contains(['\r', '\n']) {
        return Err(Error::invalid("invalid Actions output"));
    }
    writeln!(
        OpenOptions::new()
            .append(true)
            .open(environment("GITHUB_OUTPUT")?)?,
        "{name}={value}"
    )?;
    Ok(())
}

pub(crate) fn run(cli: &Cli, mode: ActionCommand) -> Result<Value> {
    guard(&|key| std::env::var(key).ok())?;
    let root = dunce::canonicalize(&cli.root)?;
    let git = Git::new(&root, environment("RCR_DEFAULT_BRANCH")?, "origin")?;
    let directory = git.artifact_directory()?;
    output(
        "directory",
        directory
            .to_str()
            .ok_or_else(|| Error::invalid("non-UTF-8 artifact path"))?,
    )?;
    if matches!(mode, ActionCommand::Guard) {
        return Ok(json!({"operation":"actions-guard","directory":directory}));
    }
    let inputs = directory.join("source-inputs.json");
    let mut command = cli.clone();
    command.artifact_directory = Some(directory);
    if command.config.is_none() {
        command.config = std::env::var_os("RCR_CONFIG")
            .filter(|value| !value.is_empty())
            .map(Into::into);
    }
    command.command = if matches!(mode, ActionCommand::Prepare) {
        Command::Prepare(ReleaseArgs {
            release_version: std::env::var("RCR_VERSION")
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| value.parse())
                .transpose()?,
            archive: None,
            inputs: None,
            save_inputs: Some(inputs),
        })
    } else {
        command.github_artifact_id = Some(
            environment("RCR_ARTIFACT_ID")?
                .parse()
                .map_err(|_| Error::invalid("invalid GitHub artifact ID"))?,
        );
        Command::Release(ReleaseArgs {
            release_version: None,
            archive: None,
            inputs: Some(inputs),
            save_inputs: None,
        })
    };
    let completed = crate::cli::run(command)?;
    for name in ["name", "version", "archive", "checksum"] {
        output(
            name,
            completed[name]
                .as_str()
                .ok_or_else(|| Error::invalid("missing release output"))?,
        )?;
    }
    let archive = completed["archive"].as_str().unwrap();
    output(
        "bundle",
        &Path::new(archive)
            .with_extension("bundle")
            .to_string_lossy(),
    )?;
    output(
        "verification",
        &serde_json::to_string(&completed["verification"])?,
    )?;
    Ok(completed)
}

/// Verifies an immutable upload from this run/attempt through GitHub's public REST API.
///
/// The ID selects real remote bytes; it is not a trusted completion receipt.
pub struct GitHubArtifacts {
    id: u64,
    api: Url,
    client: Client,
    test_server: bool,
}

#[derive(Deserialize)]
struct StoredArtifact {
    id: u64,
    name: String,
    size_in_bytes: u64,
    expired: bool,
    digest: Option<String>,
    workflow_run: StoredRun,
}

#[derive(Deserialize)]
struct StoredRun {
    id: u64,
}

impl StoredArtifact {
    fn validate(&self, id: u64, run: &str, attempt: &str) -> Result {
        if self.id != id
            || self.expired
            || self.size_in_bytes == 0
            || self.size_in_bytes > ZIP_LIMIT
            || self.workflow_run.id.to_string() != run
            || self.name != format!("rcr-release-{run}-{attempt}")
        {
            return Err(Error::invalid(
                "GitHub artifact does not match this run and attempt",
            ));
        }
        Ok(())
    }
}

impl GitHubArtifacts {
    pub fn new(id: u64) -> Result<Self> {
        Self::at(id, Url::parse("https://api.github.com").unwrap(), false)
    }

    /// Explicit loopback simulator; never reads production credentials.
    pub fn for_test_server(id: u64, endpoint: &str) -> Result<Self> {
        let url =
            Url::parse(endpoint).map_err(|_| Error::invalid("invalid artifact test endpoint"))?;
        let loopback = url
            .host_str()
            .and_then(|host| {
                host.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .ok()
            })
            .is_some_and(|address| address.is_loopback());
        if !loopback
            || !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::invalid(
                "artifact test endpoint must be a bare loopback origin",
            ));
        }
        Self::at(id, url, true)
    }

    fn at(id: u64, api: Url, test_server: bool) -> Result<Self> {
        if id == 0 {
            return Err(Error::invalid("GitHub artifact ID must be positive"));
        }
        let mut builder = Client::builder()
            .user_agent("rust-crate-release")
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(120));
        if test_server {
            builder = builder.no_proxy();
        }
        Ok(Self {
            id,
            api,
            test_server,
            client: builder
                .build()
                .map_err(|_| Error::invalid("artifact HTTP client failed"))?,
        })
    }

    fn request(&self, url: Url) -> Result<Response> {
        let value = if self.test_server {
            "rcr-loopback-test-token".into()
        } else {
            environment("GITHUB_TOKEN")?
        };
        let mut token = HeaderValue::from_str(&format!("Bearer {value}"))
            .map_err(|_| Error::invalid("invalid GitHub artifact token"))?;
        token.set_sensitive(true);
        self.client
            .get(url)
            .header(AUTHORIZATION, token)
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .map_err(|_| Error::invalid("GitHub artifact request failed"))
    }

    fn download(&self, url: Url, size: u64) -> Result<Vec<u8>> {
        let response = self.request(url.clone())?;
        let response = if response.status() == StatusCode::FOUND {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| Error::invalid("artifact download has no signed location"))?;
            let location = url
                .join(location)
                .map_err(|_| Error::invalid("invalid artifact download location"))?;
            if !location.username().is_empty()
                || location.password().is_some()
                || location.fragment().is_some()
                || if self.test_server {
                    location.origin() != self.api.origin()
                } else {
                    location.scheme() != "https"
                }
            {
                return Err(Error::invalid("unsafe artifact download location"));
            }
            // Never forward the repository token to a signed storage URL.
            self.client
                .get(location)
                .send()
                .map_err(|_| Error::invalid("GitHub artifact download failed"))?
        } else {
            response
        };
        let bytes = read_response(response, size)?;
        if bytes.len() as u64 != size {
            return Err(Error::invalid(
                "GitHub artifact download size differs from metadata",
            ));
        }
        Ok(bytes)
    }
}

fn read_response(response: Response, limit: u64) -> Result<Vec<u8>> {
    if response.status() != StatusCode::OK
        || response.content_length().is_some_and(|size| size > limit)
    {
        return Err(Error::invalid(
            "GitHub artifact response failed or exceeded its size limit",
        ));
    }
    let mut bytes = Vec::new();
    response
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::invalid("GitHub artifact response could not be read"))?;
    if bytes.len() as u64 > limit {
        return Err(Error::invalid(
            "GitHub artifact response exceeded its size limit",
        ));
    }
    Ok(bytes)
}

fn regular_bytes(path: &Path) -> Result<Vec<u8>> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(Error::invalid("preservation requires regular files"));
    }
    Ok(fs::read(path)?)
}

fn verify_snapshot(bytes: &[u8], directory: &Path, stem: &str) -> Result {
    let required = [
        format!("{stem}.crate"),
        format!("{stem}.bundle"),
        "source-inputs.json".into(),
    ];
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| Error::invalid("invalid preserved artifact ZIP"))?;
    let mut seen = BTreeSet::new();
    for index in 0..zip.len() {
        let mut file = zip
            .by_index(index)
            .map_err(|_| Error::invalid("invalid artifact ZIP entry"))?;
        let name = file
            .name()
            .map_err(|_| Error::invalid("invalid artifact ZIP filename"))?
            .into_owned();
        if !seen.insert(name.clone())
            || !required.contains(&name)
                && !matches!(name.as_str(), "release.json" | "execution.json")
            || !file.is_file()
            || file
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 != 0 && mode & 0o170000 != 0o100000)
        {
            return Err(Error::invalid(
                "unexpected or non-regular preserved artifact file",
            ));
        }
        if required.contains(&name) {
            let expected = regular_bytes(&directory.join(&name))?;
            if expected.is_empty() || file.size() != expected.len() as u64 {
                return Err(Error::invalid(
                    "preserved artifact file size differs from retained bytes",
                ));
            }
            let mut downloaded = Vec::new();
            file.take(expected.len() as u64 + 1)
                .read_to_end(&mut downloaded)?;
            if downloaded != expected {
                return Err(Error::invalid(
                    "preserved artifact file differs from retained bytes",
                ));
            }
        } else {
            if file.size() > JSON_LIMIT {
                return Err(Error::invalid(
                    "preserved diagnostic exceeds its size limit",
                ));
            }
            let mut diagnostic = Vec::new();
            (&mut file)
                .take(JSON_LIMIT + 1)
                .read_to_end(&mut diagnostic)?;
            if diagnostic.len() as u64 > JSON_LIMIT {
                return Err(Error::invalid(
                    "preserved diagnostic exceeds its size limit",
                ));
            }
        }
    }
    if required.iter().any(|name| !seen.contains(name)) {
        return Err(Error::invalid(
            "preserved artifact is missing required recovery files",
        ));
    }
    Ok(())
}

impl ArtifactPreserver for GitHubArtifacts {
    fn preserve(
        &self,
        release: &Release,
        directory: &Path,
        execution: &dyn ExecutionProvider,
    ) -> Result {
        let metadata = execution.metadata();
        if metadata.platform != "github" {
            return Err(Error::invalid("GitHub artifacts require GitHub execution"));
        }
        let repository = metadata
            .repository
            .ok_or_else(|| Error::invalid("missing artifact repository"))?;
        crate::hosting::validate_repository(&repository)?;
        let numeric = |value: Option<String>| -> Result<String> {
            value
                .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) && v != "0")
                .ok_or_else(|| Error::invalid("invalid artifact run identity"))
        };
        let run = numeric(metadata.run_id)?;
        let attempt = numeric(metadata.attempt)?;
        let name = format!("{}-{}.crate", release.name(), release.version());
        regular_bytes(&directory.join(&name))?;
        let archive = Artifact::open(&directory.join(name))?;
        if archive.name() != release.name() || archive.version() != release.version() {
            return Err(Error::invalid(
                "preserved archive differs from release identity",
            ));
        }
        let url = self
            .api
            .join(&format!(
                "/repos/{repository}/actions/artifacts/{}",
                self.id
            ))
            .map_err(|_| Error::invalid("invalid artifact API URL"))?;
        let stored: StoredArtifact =
            serde_json::from_slice(&read_response(self.request(url.clone())?, JSON_LIMIT)?)
                .map_err(|_| Error::invalid("invalid GitHub artifact metadata"))?;
        stored.validate(self.id, &run, &attempt)?;
        let download_url = self
            .api
            .join(&format!(
                "/repos/{repository}/actions/artifacts/{}/zip",
                self.id
            ))
            .map_err(|_| Error::invalid("invalid artifact download URL"))?;
        let bytes = self.download(download_url, stored.size_in_bytes)?;
        if let Some(digest) = stored.digest
            && digest != format!("sha256:{}", checksum(&bytes))
        {
            return Err(Error::invalid(
                "GitHub artifact ZIP digest differs from metadata",
            ));
        }
        verify_snapshot(
            &bytes,
            directory,
            &format!("{}-{}", release.name(), release.version()),
        )?;
        archive.verify()?;
        eprintln!("Verified durable GitHub recovery artifact before publication.");
        Ok(())
    }
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
