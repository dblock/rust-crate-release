//! Idempotent GitHub Releases and retained Cargo archive assets.

use crate::package::Artifact;
use crate::{Error, Release, Result};
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, LOCATION};
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;

const JSON_LIMIT: u64 = 2 * 1024 * 1024;
const TEST_TOKEN: &str = "rcr-loopback-test-token";

/// Hosts an already tagged release; this interface never creates or moves Git refs.
pub trait ReleaseHost: Send + Sync {
    fn publish(&self, release: &Release, commit: &str, notes: &str, artifact: &Artifact) -> Result;
}

/// The public GitHub REST API, with credentials acquired only during publication.
pub struct GitHubReleaseHost {
    repository: String,
    api: Url,
    uploads: Url,
    client: Client,
    test_server: bool,
}

struct SecretToken(String);

impl fmt::Debug for SecretToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretToken([REDACTED])")
    }
}

impl SecretToken {
    fn header(&self) -> Result<HeaderValue> {
        if self.0.is_empty() || !self.0.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(Error::invalid("GitHub authentication token is invalid"));
        }
        let mut header = HeaderValue::from_str(&format!("Bearer {}", self.0))
            .map_err(|_| Error::invalid("GitHub authentication token is invalid"))?;
        header.set_sensitive(true);
        Ok(header)
    }
}

#[derive(Deserialize)]
struct HostedRelease {
    id: u64,
    tag_name: String,
    target_commitish: String,
    name: Option<String>,
    body: Option<String>,
    draft: bool,
    prerelease: bool,
}

impl HostedRelease {
    fn validate(&self, release: &Release) -> Result {
        if self.id == 0 || self.tag_name != release.tag() {
            return Err(Error::invalid(
                "GitHub release identity does not match the requested tag",
            ));
        }
        Ok(())
    }

    fn changes(&self, release: &Release, commit: &str, notes: &str) -> Value {
        let mut changes = serde_json::Map::new();
        if self.target_commitish != commit {
            changes.insert("target_commitish".into(), json!(commit));
        }
        if self.name.as_deref() != Some(release.tag()) {
            changes.insert("name".into(), json!(release.tag()));
        }
        if self.body.as_deref() != Some(notes) {
            changes.insert("body".into(), json!(notes));
        }
        if self.draft {
            changes.insert("draft".into(), json!(false));
        }
        if self.prerelease {
            changes.insert("prerelease".into(), json!(false));
        }
        Value::Object(changes)
    }

    fn matches(&self, release: &Release, commit: &str, notes: &str) -> bool {
        self.changes(release, commit, notes)
            .as_object()
            .unwrap()
            .is_empty()
    }
}

#[derive(Clone, Deserialize)]
struct HostedAsset {
    id: u64,
    name: String,
    size: u64,
    state: String,
    digest: Option<String>,
}

impl HostedAsset {
    fn validate(&self) -> Result {
        if self.id == 0
            || self.name.is_empty()
            || !matches!(self.state.as_str(), "uploaded" | "starter")
        {
            return Err(Error::invalid("GitHub asset metadata is invalid"));
        }
        if let Some(digest) = &self.digest {
            let checksum = digest.strip_prefix("sha256:").ok_or_else(|| {
                Error::invalid("GitHub asset digest is not a supported SHA-256 digest")
            })?;
            if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(Error::invalid("GitHub asset SHA-256 digest is malformed"));
            }
        }
        Ok(())
    }
}

impl GitHubReleaseHost {
    /// Selects `owner/name` on github.com, without reading credentials or contacting GitHub.
    pub fn new(repository: impl Into<String>) -> Result<Self> {
        let repository = repository.into();
        validate_repository(&repository)?;
        Ok(Self {
            repository,
            api: Url::parse("https://api.github.com")
                .map_err(|_| Error::invalid("invalid built-in GitHub API endpoint"))?,
            uploads: Url::parse("https://uploads.github.com")
                .map_err(|_| Error::invalid("invalid built-in GitHub upload endpoint"))?,
            client: client(false)?,
            test_server: false,
        })
    }

    /// An isolated simulator with a fixed dummy token; never reads environment or gh credentials.
    ///
    /// The endpoint must be a bare HTTP(S) origin with a literal loopback IP address.
    pub fn for_test_server(repository: impl Into<String>, endpoint: &str) -> Result<Self> {
        let repository = repository.into();
        validate_repository(&repository)?;
        let endpoint =
            Url::parse(endpoint).map_err(|_| Error::invalid("invalid GitHub test endpoint"))?;
        let loopback = endpoint
            .host_str()
            .and_then(|host| {
                host.trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<std::net::IpAddr>()
                    .ok()
            })
            .is_some_and(|address| address.is_loopback());
        if !loopback
            || !matches!(endpoint.scheme(), "http" | "https")
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.path() != "/"
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(Error::invalid(
                "GitHub test endpoint must be a bare loopback origin",
            ));
        }
        Ok(Self {
            repository,
            api: endpoint.clone(),
            uploads: endpoint,
            client: client(true)?,
            test_server: true,
        })
    }

    fn url(&self, suffix: &str) -> Result<Url> {
        self.api
            .join(&format!("/repos/{}/releases{suffix}", self.repository))
            .map_err(|_| Error::invalid("invalid GitHub API path"))
    }

    fn request(&self, request: RequestBuilder, token: &SecretToken) -> Result<RequestBuilder> {
        Ok(request
            .header(AUTHORIZATION, token.header()?)
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28"))
    }

    fn token(&self) -> Result<SecretToken> {
        if self.test_server {
            return Ok(SecretToken(TEST_TOKEN.into()));
        }
        for name in ["GH_TOKEN", "GITHUB_TOKEN"] {
            match std::env::var(name) {
                Ok(value) if !value.is_empty() => {
                    let token = SecretToken(value);
                    token.header()?;
                    return Ok(token);
                }
                Err(std::env::VarError::NotUnicode(_)) => {
                    return Err(Error::invalid("GitHub authentication token is invalid"));
                }
                _ => {}
            }
        }
        let output = Command::new("gh")
            .args(["auth", "token", "--hostname", "github.com"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map_err(|_| {
                Error::invalid("GitHub authentication requires GH_TOKEN, GITHUB_TOKEN, or gh auth")
            })?;
        if !output.status.success() {
            return Err(Error::invalid(
                "GitHub authentication requires GH_TOKEN, GITHUB_TOKEN, or gh auth",
            ));
        }
        let value = String::from_utf8(output.stdout)
            .map_err(|_| Error::invalid("GitHub authentication token is invalid"))?;
        let token = SecretToken(value.trim_end_matches(['\r', '\n']).into());
        token.header()?;
        Ok(token)
    }

    fn lookup(&self, release: &Release, token: &SecretToken) -> Result<Option<HostedRelease>> {
        let request = self.request(
            self.client
                .get(self.url(&format!("/tags/{}", release.tag()))?),
            token,
        )?;
        let response = send(request, "GitHub release lookup")?;
        if response.status() == StatusCode::NOT_FOUND {
            let body = response_json(response, "GitHub release lookup")?;
            if body.get("message").and_then(Value::as_str) == Some("Not Found") {
                return Ok(None);
            }
            return Err(Error::invalid(
                "GitHub release lookup returned an unrecognized 404 response",
            ));
        }
        expect_status(&response, StatusCode::OK, "GitHub release lookup")?;
        let hosted: HostedRelease = decode(response, "GitHub release lookup")?;
        hosted.validate(release)?;
        Ok(Some(hosted))
    }

    fn assets(&self, release_id: u64, token: &SecretToken) -> Result<Vec<HostedAsset>> {
        let mut assets = Vec::new();
        let mut ids = BTreeSet::new();
        for page in 1..=100 {
            let request = self.request(
                self.client
                    .get(self.url(&format!("/{release_id}/assets"))?)
                    .query(&[("per_page", 100), ("page", page)]),
                token,
            )?;
            let response = send(request, "GitHub asset listing")?;
            expect_status(&response, StatusCode::OK, "GitHub asset listing")?;
            let listed: Vec<HostedAsset> = decode(response, "GitHub asset listing")?;
            for asset in &listed {
                asset.validate()?;
                if !ids.insert(asset.id) {
                    return Err(Error::invalid(
                        "GitHub asset listing contains duplicate IDs",
                    ));
                }
            }
            let complete = listed.len() < 100;
            assets.extend(listed);
            if complete {
                return Ok(assets);
            }
        }
        Err(Error::invalid(
            "GitHub asset listing exceeded the bounded page limit",
        ))
    }

    fn create(
        &self,
        release: &Release,
        commit: &str,
        notes: &str,
        token: &SecretToken,
    ) -> Result<HostedRelease> {
        let request = self.request(
            self.client.post(self.url("")?).json(&json!({
                "tag_name": release.tag(),
                "target_commitish": commit,
                "name": release.tag(),
                "body": notes,
                "draft": false,
                "prerelease": false,
            })),
            token,
        )?;
        let response = send(request, "GitHub release creation")?;
        expect_status(&response, StatusCode::CREATED, "GitHub release creation")?;
        let hosted: HostedRelease = decode(response, "GitHub release creation")?;
        hosted.validate(release)?;
        if !hosted.matches(release, commit, notes) {
            return Err(Error::invalid(
                "created GitHub release metadata does not match the request",
            ));
        }
        Ok(hosted)
    }

    fn patch(
        &self,
        hosted: &HostedRelease,
        release: &Release,
        commit: &str,
        notes: &str,
        token: &SecretToken,
    ) -> Result {
        let changes = hosted.changes(release, commit, notes);
        if changes.as_object().unwrap().is_empty() {
            return Ok(());
        }
        let request = self.request(
            self.client
                .patch(self.url(&format!("/{}", hosted.id))?)
                .json(&changes),
            token,
        )?;
        let response = send(request, "GitHub release update")?;
        expect_status(&response, StatusCode::OK, "GitHub release update")?;
        let updated: HostedRelease = decode(response, "GitHub release update")?;
        updated.validate(release)?;
        if updated.id != hosted.id || !updated.matches(release, commit, notes) {
            return Err(Error::invalid(
                "updated GitHub release metadata does not match the request",
            ));
        }
        Ok(())
    }

    fn asset_matches(
        &self,
        asset: &HostedAsset,
        checksum: &str,
        length: u64,
        token: &SecretToken,
    ) -> Result<bool> {
        if asset.state != "uploaded" || asset.size != length {
            return Ok(false);
        }
        if let Some(digest) = &asset.digest {
            return Ok(digest[7..].eq_ignore_ascii_case(checksum));
        }
        let bytes = self.download(asset.id, length, token)?;
        Ok(sha256(&bytes).eq_ignore_ascii_case(checksum))
    }

    fn download(&self, asset_id: u64, length: u64, token: &SecretToken) -> Result<Vec<u8>> {
        let mut url = self.url(&format!("/assets/{asset_id}"))?;
        let request = self
            .client
            .get(url.clone())
            .header(AUTHORIZATION, token.header()?)
            .header(ACCEPT, "application/octet-stream")
            .header("X-GitHub-Api-Version", "2022-11-28");
        let mut response = send(request, "GitHub asset download")?;
        for _ in 0..5 {
            if response.status() == StatusCode::OK {
                return read_bounded(response, length, "GitHub asset download");
            }
            if !matches!(
                response.status(),
                StatusCode::MOVED_PERMANENTLY
                    | StatusCode::FOUND
                    | StatusCode::SEE_OTHER
                    | StatusCode::TEMPORARY_REDIRECT
                    | StatusCode::PERMANENT_REDIRECT
            ) {
                return Err(status_error(&response, "GitHub asset download"));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    Error::invalid("GitHub asset download redirect has no valid location")
                })?;
            let next = url
                .join(location)
                .map_err(|_| Error::invalid("GitHub asset download redirect is invalid"))?;
            if !next.username().is_empty()
                || next.password().is_some()
                || next.fragment().is_some()
                || if self.test_server {
                    next.origin() != self.api.origin()
                } else {
                    next.scheme() != "https"
                }
            {
                return Err(Error::invalid(
                    "GitHub asset download redirect has an unsafe origin",
                ));
            }
            url = next;
            // A signed location gets a fresh GET, never the API token or an upload body.
            response = send(
                self.client
                    .get(url.clone())
                    .header(ACCEPT, "application/octet-stream"),
                "GitHub asset download",
            )?;
        }
        Err(Error::invalid(
            "GitHub asset download exceeded the redirect limit",
        ))
    }

    fn remove_asset(&self, asset: &HostedAsset, token: &SecretToken) -> Result {
        let request = self.request(
            self.client
                .delete(self.url(&format!("/assets/{}", asset.id))?),
            token,
        )?;
        let response = send(request, "GitHub asset deletion")?;
        expect_status(&response, StatusCode::NO_CONTENT, "GitHub asset deletion")
    }

    fn upload(&self, release_id: u64, name: &str, bytes: Vec<u8>, token: &SecretToken) -> Result {
        // Never trust the upload_url returned in release metadata.
        let endpoint = self
            .uploads
            .join(&format!(
                "/repos/{}/releases/{release_id}/assets",
                self.repository
            ))
            .map_err(|_| Error::invalid("invalid GitHub asset upload path"))?;
        let request = self.request(
            self.client
                .post(endpoint)
                .query(&[("name", name)])
                .header(CONTENT_TYPE, "application/octet-stream")
                .body(bytes),
            token,
        )?;
        let response = send(request, "GitHub asset upload")?;
        expect_status(&response, StatusCode::CREATED, "GitHub asset upload")?;
        let uploaded: HostedAsset = decode(response, "GitHub asset upload")?;
        uploaded.validate()?;
        if uploaded.name != name {
            return Err(Error::invalid(
                "uploaded GitHub asset has an unexpected name",
            ));
        }
        Ok(())
    }
}

impl ReleaseHost for GitHubReleaseHost {
    fn publish(&self, release: &Release, commit: &str, notes: &str, artifact: &Artifact) -> Result {
        if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Error::invalid(
                "hosting requires a complete Git commit object ID",
            ));
        }
        if artifact.name() != release.name() || artifact.version() != release.version() {
            return Err(Error::invalid(
                "hosted archive identity does not match the captured release",
            ));
        }
        artifact.verify()?;
        let bytes = std::fs::read(artifact.path())?;
        let checksum = sha256(&bytes);
        if checksum != artifact.checksum() {
            return Err(Error::invalid("retained archive changed before hosting"));
        }
        let length = bytes.len() as u64;
        let name = format!("{}-{}.crate", release.name(), release.version());
        let token = self.token()?;
        let hosted = match self.lookup(release, &token)? {
            Some(hosted) => hosted,
            None => self.create(release, commit, notes, &token)?,
        };
        let assets = self.assets(hosted.id, &token)?;
        let existing = canonical_asset(&assets, &name)?;
        self.patch(&hosted, release, commit, notes, &token)?;
        let reuse = match existing {
            Some(asset) => self.asset_matches(asset, &checksum, length, &token)?,
            None => false,
        };
        if !reuse {
            if let Some(asset) = existing {
                self.remove_asset(asset, &token)?;
            }
            self.upload(hosted.id, &name, bytes, &token)?;
        }
        let fresh = self
            .lookup(release, &token)?
            .ok_or_else(|| Error::invalid("GitHub release is missing after hosting"))?;
        if fresh.id != hosted.id || !fresh.matches(release, commit, notes) {
            return Err(Error::invalid(
                "fresh GitHub release metadata does not match the requested release",
            ));
        }
        let assets = self.assets(fresh.id, &token)?;
        let asset = canonical_asset(&assets, &name)?
            .ok_or_else(|| Error::invalid("GitHub archive asset is missing after hosting"))?;
        if !self.asset_matches(asset, &checksum, length, &token)? {
            return Err(Error::invalid(
                "fresh GitHub archive asset does not match retained bytes",
            ));
        }
        Ok(())
    }
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn validate_repository(repository: &str) -> Result {
    let invalid = || Error::invalid("GitHub repository must be a literal owner/name");
    let (owner, name) = repository.split_once('/').ok_or_else(invalid)?;
    if owner.is_empty()
        || owner.len() > 39
        || !owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || owner.starts_with('-')
        || owner.ends_with('-')
        || name.is_empty()
        || name.len() > 100
        || matches!(name, "." | "..")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(invalid());
    }
    Ok(())
}

fn canonical_asset<'a>(assets: &'a [HostedAsset], name: &str) -> Result<Option<&'a HostedAsset>> {
    let mut matching = assets.iter().filter(|asset| asset.name == name);
    let first = matching.next();
    if matching.next().is_some() {
        return Err(Error::invalid(
            "GitHub release contains duplicate canonical archive assets",
        ));
    }
    Ok(first)
}

fn client(test_server: bool) -> Result<Client> {
    Client::builder()
        .user_agent("rust-crate-release")
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .referer(false)
        .no_proxy()
        .https_only(!test_server)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| Error::invalid("GitHub HTTP client initialization failed"))
}

fn send(request: RequestBuilder, operation: &str) -> Result<Response> {
    request.send().map_err(|error| {
        let _ = error.without_url();
        Error::invalid(format!(
            "{operation} request failed; reconcile hosted state before retrying"
        ))
    })
}

fn status_error(response: &Response, operation: &str) -> Error {
    Error::invalid(format!(
        "{operation} failed with HTTP {}",
        response.status().as_u16()
    ))
}

fn expect_status(response: &Response, expected: StatusCode, operation: &str) -> Result {
    if response.status() != expected {
        return Err(status_error(response, operation));
    }
    Ok(())
}

fn read_bounded(response: Response, limit: u64, operation: &str) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(Error::invalid(format!(
            "{operation} response exceeds the expected size"
        )));
    }
    let mut bytes = Vec::new();
    response
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| Error::invalid(format!("{operation} response could not be read")))?;
    if bytes.len() as u64 > limit {
        return Err(Error::invalid(format!(
            "{operation} response exceeds the expected size"
        )));
    }
    Ok(bytes)
}

fn response_json(response: Response, operation: &str) -> Result<Value> {
    let bytes = read_bounded(response, JSON_LIMIT, operation)?;
    serde_json::from_slice(&bytes)
        .map_err(|_| Error::invalid(format!("{operation} response is not valid JSON")))
}

fn decode<T: serde::de::DeserializeOwned>(response: Response, operation: &str) -> Result<T> {
    serde_json::from_value(response_json(response, operation)?)
        .map_err(|_| Error::invalid(format!("{operation} response metadata is malformed")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{Git, ReleaseRepository};
    use crate::package::{CargoPackage, PackageProject};
    use crate::testing::GitProject;
    use semver::Version;
    use std::collections::BTreeMap;
    use std::io::{self, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};

    const REPOSITORY: &str = "owner/repository";
    const ASSET_NAME: &str = "release-fixture-0.1.0.crate";
    const NOTES: &str = "## Changes\n\n- Retain the checked Cargo archive.\n";

    struct Mutation {
        method: String,
        path: String,
        body: Vec<u8>,
    }

    #[derive(Default)]
    struct Simulation {
        release: Option<Value>,
        assets: Vec<Value>,
        payloads: BTreeMap<u64, Vec<u8>>,
        mutations: Vec<Mutation>,
        requests: usize,
        lookup_failure: Option<(u16, Vec<u8>)>,
        redirect_downloads: bool,
        unsafe_redirect: Option<String>,
        download_auth_leaked: bool,
        api_authenticated: bool,
        downloads: usize,
        upload_without_digest: bool,
        lose_upload_response: bool,
        corrupt_uploaded_metadata: bool,
        next_id: u64,
    }

    struct Server {
        endpoint: String,
        state: Arc<Mutex<Simulation>>,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl Server {
        fn new() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let state = Arc::new(Mutex::new(Simulation {
                api_authenticated: true,
                next_id: 1000,
                ..Simulation::default()
            }));
            let stop = Arc::new(AtomicBool::new(false));
            let worker_state = Arc::clone(&state);
            let worker_stop = Arc::clone(&stop);
            let worker_endpoint = endpoint.clone();
            let worker = thread::spawn(move || {
                while !worker_stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream.set_nonblocking(false).unwrap();
                            stream
                                .set_read_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            stream
                                .set_write_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            if let Ok(Some(request)) = read_request(&mut stream) {
                                let response = respond(
                                    &request,
                                    &mut worker_state.lock().unwrap(),
                                    &worker_endpoint,
                                );
                                let _ = write_response(&mut stream, response);
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                endpoint,
                state,
                stop,
                worker: Some(worker),
            }
        }

        fn host(&self) -> GitHubReleaseHost {
            GitHubReleaseHost::for_test_server(REPOSITORY, &self.endpoint).unwrap()
        }

        fn edit<T>(&self, edit: impl FnOnce(&mut Simulation) -> T) -> T {
            edit(&mut self.state.lock().unwrap())
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let address = self.endpoint.trim_start_matches("http://").parse().unwrap();
            let _ = TcpStream::connect_timeout(&address, Duration::from_secs(1));
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    struct Request {
        method: String,
        target: String,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    }

    fn read_request(stream: &mut TcpStream) -> io::Result<Option<Request>> {
        let mut bytes = Vec::new();
        let end = loop {
            let mut chunk = [0; 4096];
            let count = stream.read(&mut chunk)?;
            if count == 0 {
                return Ok(None);
            }
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                break end + 4;
            }
            if bytes.len() > 64 * 1024 {
                return Err(io::Error::other("simulator request headers are too large"));
            }
        };
        let head = std::str::from_utf8(&bytes[..end])
            .map_err(|_| io::Error::other("invalid simulator request headers"))?;
        let mut lines = head.split("\r\n");
        let mut first = lines.next().unwrap().split_whitespace();
        let method = first.next().unwrap_or_default().into();
        let target = first.next().unwrap_or_default().into();
        let headers: BTreeMap<_, _> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        let length: usize = headers
            .get("content-length")
            .map(|value| value.parse())
            .transpose()
            .map_err(|_| io::Error::other("invalid simulator request length"))?
            .unwrap_or_default();
        if length > 8 * 1024 * 1024 {
            return Err(io::Error::other("simulator request body is too large"));
        }
        while bytes.len() - end < length {
            let mut chunk = [0; 4096];
            let count = stream.read(&mut chunk)?;
            if count == 0 {
                return Err(io::Error::other("incomplete simulator request"));
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
        Ok(Some(Request {
            method,
            target,
            headers,
            body: bytes[end..end + length].to_vec(),
        }))
    }

    struct Reply {
        status: u16,
        body: Vec<u8>,
        location: Option<String>,
    }

    fn reply(status: u16, body: Value) -> Reply {
        Reply {
            status,
            body: serde_json::to_vec(&body).unwrap(),
            location: None,
        }
    }

    fn write_response(stream: &mut TcpStream, response: Reply) -> io::Result<()> {
        write!(
            stream,
            "HTTP/1.1 {} Simulator\r\nContent-Length: {}\r\nConnection: close\r\n",
            response.status,
            response.body.len()
        )?;
        if let Some(location) = response.location {
            write!(stream, "Location: {location}\r\n")?;
        }
        stream.write_all(b"\r\n")?;
        stream.write_all(&response.body)
    }

    fn respond(request: &Request, state: &mut Simulation, endpoint: &str) -> Reply {
        state.requests += 1;
        let url = Url::parse(&format!("{endpoint}{}", request.target)).unwrap();
        let path = url.path();
        if let Some(id) = path.strip_prefix("/download/") {
            state.download_auth_leaked |= request.headers.contains_key("authorization");
            state.downloads += 1;
            return Reply {
                status: 200,
                body: state.payloads[&id.parse::<u64>().unwrap()].clone(),
                location: None,
            };
        }
        if request.headers.get("authorization").map(String::as_str)
            != Some(&format!("Bearer {TEST_TOKEN}"))
        {
            state.api_authenticated = false;
            return reply(401, json!({"message": "Unauthorized"}));
        }
        let suffix = path
            .strip_prefix("/repos/owner/repository/releases")
            .unwrap_or("");
        if request.method == "GET" && suffix.starts_with("/tags/") {
            if let Some((status, body)) = &state.lookup_failure {
                return Reply {
                    status: *status,
                    body: body.clone(),
                    location: None,
                };
            }
            return match &state.release {
                Some(release) => reply(200, release.clone()),
                None => reply(404, json!({"message": "Not Found"})),
            };
        }
        if request.method == "GET" && suffix == "/7/assets" {
            let page: usize = url
                .query_pairs()
                .find(|(key, _)| key == "page")
                .map(|(_, value)| value.parse().unwrap())
                .unwrap_or(1);
            return reply(
                200,
                Value::Array(
                    state
                        .assets
                        .iter()
                        .skip((page - 1) * 100)
                        .take(100)
                        .cloned()
                        .collect(),
                ),
            );
        }
        if request.method == "GET" && suffix.starts_with("/assets/") {
            let id: u64 = suffix.trim_start_matches("/assets/").parse().unwrap();
            if request.headers.get("accept").map(String::as_str) != Some("application/octet-stream")
            {
                return reply(406, json!({"message": "An octet stream was not requested"}));
            }
            if let Some(location) = &state.unsafe_redirect {
                return Reply {
                    status: 302,
                    body: Vec::new(),
                    location: Some(location.clone()),
                };
            }
            if state.redirect_downloads {
                return Reply {
                    status: 302,
                    body: Vec::new(),
                    location: Some(format!("{endpoint}/download/{id}?signature=dummy")),
                };
            }
            state.downloads += 1;
            return Reply {
                status: 200,
                body: state.payloads[&id].clone(),
                location: None,
            };
        }
        if !matches!(request.method.as_str(), "POST" | "PATCH" | "DELETE") {
            return reply(404, json!({"message": "Unknown simulator operation"}));
        }
        state.mutations.push(Mutation {
            method: request.method.clone(),
            path: request.target.clone(),
            body: request.body.clone(),
        });
        if request.method == "POST" && suffix.is_empty() {
            let mut release: Value = serde_json::from_slice(&request.body).unwrap();
            release["id"] = json!(7);
            release["upload_url"] = json!("https://untrusted.invalid/never-upload-here{?name}");
            state.release = Some(release.clone());
            return reply(201, release);
        }
        if request.method == "PATCH" && suffix == "/7" {
            let changes: Value = serde_json::from_slice(&request.body).unwrap();
            let release = state.release.as_mut().unwrap();
            for (key, value) in changes.as_object().unwrap() {
                release[key] = value.clone();
            }
            return reply(200, release.clone());
        }
        if request.method == "DELETE" && suffix.starts_with("/assets/") {
            let id: u64 = suffix.trim_start_matches("/assets/").parse().unwrap();
            state
                .assets
                .retain(|asset| asset["id"].as_u64() != Some(id));
            state.payloads.remove(&id);
            return Reply {
                status: 204,
                body: Vec::new(),
                location: None,
            };
        }
        if request.method == "POST" && suffix == "/7/assets" {
            let name = url
                .query_pairs()
                .find(|(key, _)| key == "name")
                .unwrap()
                .1
                .into_owned();
            state.next_id += 1;
            let id = state.next_id;
            let asset = asset_json(id, &name, &request.body, !state.upload_without_digest);
            let mut stored = asset.clone();
            if state.corrupt_uploaded_metadata {
                stored["digest"] = json!(format!("sha256:{}", "0".repeat(64)));
            }
            state.assets.push(stored);
            state.payloads.insert(id, request.body.clone());
            if state.lose_upload_response {
                state.lose_upload_response = false;
                return reply(500, json!({"message": "private-response-body"}));
            }
            return reply(201, asset);
        }
        reply(404, json!({"message": "Unknown simulator mutation"}))
    }

    fn asset_json(id: u64, name: &str, bytes: &[u8], digest: bool) -> Value {
        json!({
            "id": id,
            "name": name,
            "size": bytes.len(),
            "state": "uploaded",
            "digest": if digest {
                Some(format!("sha256:{}", sha256(bytes)))
            } else {
                None
            },
        })
    }

    fn release_json(release: &Release, commit: &str, notes: &str) -> Value {
        json!({
            "id": 7,
            "tag_name": release.tag(),
            "target_commitish": commit,
            "name": release.tag(),
            "body": notes,
            "draft": false,
            "prerelease": false,
        })
    }

    fn fixture() -> (GitProject, Artifact, Release, String) {
        let project = GitProject::cargo_library().unwrap();
        let source = Git::new(project.root(), "master", "origin")
            .unwrap()
            .inspect()
            .unwrap()
            .head;
        let artifact = CargoPackage::new(project.root())
            .unwrap()
            .build(
                &project.temporary_directory().join("hosting-artifacts"),
                1_762_000_000,
            )
            .unwrap();
        let release = Release::new(
            artifact.name(),
            artifact.version().clone(),
            artifact.version().clone(),
            source,
            "2026/11/12",
        )
        .unwrap();
        (project, artifact, release, "b".repeat(40))
    }

    #[test]
    fn creates_release_and_uploads_exact_retained_bytes_then_reuses_everything() {
        let (_project, artifact, release, commit) = fixture();
        let bytes = std::fs::read(artifact.path()).unwrap();
        let server = Server::new();
        let host = server.host();
        host.publish(&release, &commit, NOTES, &artifact).unwrap();
        server.edit(|state| {
            assert!(state.api_authenticated);
            assert_eq!(state.mutations.len(), 2);
            let creation: Value = serde_json::from_slice(&state.mutations[0].body).unwrap();
            assert_eq!(
                creation,
                json!({
                    "tag_name": release.tag(),
                    "target_commitish": commit,
                    "name": release.tag(),
                    "body": NOTES,
                    "draft": false,
                    "prerelease": false,
                })
            );
            assert_eq!(state.mutations[1].body, bytes);
            assert_eq!(state.assets[0]["name"], ASSET_NAME);
            assert_eq!(state.payloads[&1001], bytes);
            assert_eq!(
                state.mutations[1].path,
                format!("/repos/{REPOSITORY}/releases/7/assets?name={ASSET_NAME}")
            );
        });
        host.publish(&release, &commit, NOTES, &artifact).unwrap();
        server.edit(|state| assert_eq!(state.mutations.len(), 2));
    }

    #[test]
    fn patches_only_changed_metadata_and_replaces_only_the_differing_archive() {
        let (_project, artifact, release, commit) = fixture();
        let bytes = std::fs::read(artifact.path()).unwrap();
        let server = Server::new();
        server.edit(|state| {
            let mut hosted = release_json(&release, &"a".repeat(40), "Old notes");
            hosted["draft"] = json!(true);
            state.release = Some(hosted);
            state.assets = vec![
                asset_json(8, ASSET_NAME, b"old archive", true),
                asset_json(9, "unrelated.txt", b"preserve me", true),
            ];
            state.payloads.insert(8, b"old archive".to_vec());
            state.payloads.insert(9, b"preserve me".to_vec());
        });
        server
            .host()
            .publish(&release, &commit, NOTES, &artifact)
            .unwrap();
        server.edit(|state| {
            assert_eq!(
                state
                    .mutations
                    .iter()
                    .map(|mutation| mutation.method.as_str())
                    .collect::<Vec<_>>(),
                ["PATCH", "DELETE", "POST"]
            );
            let changes: Value = serde_json::from_slice(&state.mutations[0].body).unwrap();
            assert_eq!(
                changes,
                json!({
                    "target_commitish": commit,
                    "body": NOTES,
                    "draft": false,
                })
            );
            assert_eq!(
                state.mutations[1].path,
                "/repos/owner/repository/releases/assets/8"
            );
            assert_eq!(state.assets.len(), 2);
            assert_eq!(state.payloads[&9], b"preserve me");
            assert_eq!(state.payloads[&1001], bytes);
            assert!(!state.payloads.contains_key(&8));
        });
    }

    #[test]
    fn missing_digests_download_bytes_without_forwarding_authentication_to_signed_locations() {
        let (_project, artifact, release, commit) = fixture();
        let bytes = std::fs::read(artifact.path()).unwrap();
        let server = Server::new();
        server.edit(|state| {
            state.release = Some(release_json(&release, &commit, NOTES));
            state.assets.push(asset_json(8, ASSET_NAME, &bytes, false));
            state.payloads.insert(8, bytes);
            state.redirect_downloads = true;
        });
        server
            .host()
            .publish(&release, &commit, NOTES, &artifact)
            .unwrap();
        server.edit(|state| {
            assert!(state.api_authenticated);
            assert!(!state.download_auth_leaked);
            assert_eq!(state.downloads, 2);
            assert!(state.mutations.is_empty());
        });
    }

    #[test]
    fn absent_digests_still_detect_different_bytes_and_verify_fresh_uploads() {
        let (_project, artifact, release, commit) = fixture();
        let bytes = std::fs::read(artifact.path()).unwrap();
        let mut different = bytes.clone();
        different[0] ^= 1;
        let server = Server::new();
        server.edit(|state| {
            state.release = Some(release_json(&release, &commit, NOTES));
            state
                .assets
                .push(asset_json(8, ASSET_NAME, &different, false));
            state.payloads.insert(8, different);
            state.upload_without_digest = true;
        });
        server
            .host()
            .publish(&release, &commit, NOTES, &artifact)
            .unwrap();
        server.edit(|state| {
            assert_eq!(state.mutations.len(), 2);
            assert_eq!(state.mutations[0].method, "DELETE");
            assert_eq!(state.mutations[1].method, "POST");
            assert_eq!(state.payloads[&1001], bytes);
            assert_eq!(state.downloads, 2);
        });
    }

    #[test]
    fn only_a_structured_not_found_response_authorizes_creation() {
        let (_project, artifact, release, commit) = fixture();
        let invalid_release = release_json(&release, &commit, NOTES);
        let mut wrong_tag = invalid_release.clone();
        wrong_tag["tag_name"] = json!("v9.0.0");
        let mut zero_id = invalid_release.clone();
        zero_id["id"] = json!(0);
        for (status, body) in [
            (401, br#"{"message":"private-response-body"}"#.to_vec()),
            (403, br#"{"message":"private-response-body"}"#.to_vec()),
            (404, br#"{"message":"Not an authorized absence"}"#.to_vec()),
            (404, b"private-response-body".to_vec()),
            (200, b"private-response-body".to_vec()),
            (200, br#"{"id":7}"#.to_vec()),
            (200, serde_json::to_vec(&wrong_tag).unwrap()),
            (200, serde_json::to_vec(&zero_id).unwrap()),
        ] {
            let server = Server::new();
            server.edit(|state| state.lookup_failure = Some((status, body)));
            let error = server
                .host()
                .publish(&release, &commit, NOTES, &artifact)
                .unwrap_err();
            let diagnostic = format!("{error} {error:?}");
            assert!(!diagnostic.contains("private-response-body"));
            assert!(!diagnostic.contains(TEST_TOKEN));
            server.edit(|state| {
                assert_eq!(state.requests, 1);
                assert!(state.mutations.is_empty());
            });
        }
    }

    #[test]
    fn duplicate_canonical_assets_across_pages_fail_before_metadata_or_asset_mutation() {
        let (_project, artifact, release, commit) = fixture();
        let bytes = std::fs::read(artifact.path()).unwrap();
        let server = Server::new();
        server.edit(|state| {
            state.release = Some(release_json(&release, &commit, "Needs a patch"));
            state.assets.push(asset_json(8, ASSET_NAME, &bytes, true));
            for id in 10..109 {
                state
                    .assets
                    .push(asset_json(id, &format!("other-{id}"), b"other", true));
            }
            state.assets.push(asset_json(9, ASSET_NAME, &bytes, true));
        });
        let error = server
            .host()
            .publish(&release, &commit, NOTES, &artifact)
            .unwrap_err();
        assert!(error.to_string().contains("duplicate canonical"));
        server.edit(|state| {
            assert_eq!(state.requests, 3);
            assert!(state.mutations.is_empty());
        });
    }

    #[test]
    fn invalid_identity_commit_and_modified_archive_fail_before_any_http_request() {
        let (_project, artifact, release, commit) = fixture();
        let server = Server::new();
        let host = server.host();
        for (name, version) in [
            (release.name(), Version::new(0, 2, 0)),
            ("another-package", release.version().clone()),
        ] {
            let wrong = Release::new(
                name,
                version,
                release.manifest_version().clone(),
                release.source_commit(),
                release.date(),
            )
            .unwrap();
            assert!(host.publish(&wrong, &commit, NOTES, &artifact).is_err());
        }
        assert!(host.publish(&release, "HEAD", NOTES, &artifact).is_err());
        let mut bytes = std::fs::read(artifact.path()).unwrap();
        bytes.push(0);
        std::fs::write(artifact.path(), bytes).unwrap();
        assert!(host.publish(&release, &commit, NOTES, &artifact).is_err());
        server.edit(|state| assert_eq!(state.requests, 0));
    }

    #[test]
    fn repositories_and_simulator_endpoints_cannot_inject_paths_or_select_external_hosts() {
        for repository in [
            "",
            "owner",
            "owner/repository/extra",
            "../repository",
            "owner/..",
            "owner/%2fother",
            "owner/repository?query",
            "owner/repository#fragment",
            "-owner/repository",
            "owner/repository\n",
        ] {
            assert!(GitHubReleaseHost::new(repository).is_err());
            assert!(GitHubReleaseHost::for_test_server(repository, "http://127.0.0.1").is_err());
        }
        for endpoint in [
            "https://api.github.com",
            "http://example.invalid",
            "http://localhost",
            "file:///127.0.0.1",
            "http://127.0.0.1/path",
            "http://127.0.0.1?query",
            "http://127.0.0.1#fragment",
            "http://private-token@127.0.0.1",
        ] {
            assert!(GitHubReleaseHost::for_test_server(REPOSITORY, endpoint).is_err());
        }
        assert!(GitHubReleaseHost::new(REPOSITORY).is_ok());
        assert!(GitHubReleaseHost::for_test_server(REPOSITORY, "http://[::1]:8080").is_ok());
    }

    #[test]
    fn unsafe_signed_locations_and_secret_token_diagnostics_are_redacted() {
        let (_project, artifact, release, commit) = fixture();
        let bytes = std::fs::read(artifact.path()).unwrap();
        for location in [
            "http://example.invalid/archive?private-signed-token",
            "https://example.invalid/archive?private-signed-token",
            "http://private-signed-token@127.0.0.1/archive",
        ] {
            let server = Server::new();
            server.edit(|state| {
                state.release = Some(release_json(&release, &commit, NOTES));
                state.assets.push(asset_json(8, ASSET_NAME, &bytes, false));
                state.payloads.insert(8, bytes.clone());
                state.unsafe_redirect = Some(location.into());
            });
            let error = server
                .host()
                .publish(&release, &commit, NOTES, &artifact)
                .unwrap_err();
            let diagnostic = format!("{error} {error:?}");
            assert!(
                diagnostic.contains("unsafe origin"),
                "expected an unsafe-origin rejection, got: {diagnostic}"
            );
            assert!(!diagnostic.contains("private-signed-token"));
            assert!(!diagnostic.contains(TEST_TOKEN));
            server.edit(|state| assert!(state.mutations.is_empty()));
        }
        let token = SecretToken("private-token-value".into());
        assert!(!format!("{token:?}").contains("private-token-value"));
        let invalid = SecretToken("private-token-value\ninjected".into());
        let error = invalid.header().unwrap_err();
        assert!(!format!("{error} {error:?}").contains("private-token-value"));
    }

    #[test]
    fn ambiguous_upload_is_not_retried_and_the_next_call_reconciles_hosted_state() {
        let (_project, artifact, release, commit) = fixture();
        let server = Server::new();
        server.edit(|state| state.lose_upload_response = true);
        let host = server.host();
        let error = host
            .publish(&release, &commit, NOTES, &artifact)
            .unwrap_err();
        assert!(error.to_string().contains("HTTP 500"));
        assert!(!format!("{error:?}").contains("private-response-body"));
        server.edit(|state| {
            assert_eq!(state.mutations.len(), 2);
            assert_eq!(state.assets.len(), 1);
        });
        host.publish(&release, &commit, NOTES, &artifact).unwrap();
        server.edit(|state| assert_eq!(state.mutations.len(), 2));
    }

    #[test]
    fn fresh_metadata_must_match_even_after_a_successful_upload_response() {
        let (_project, artifact, release, commit) = fixture();
        let server = Server::new();
        server.edit(|state| state.corrupt_uploaded_metadata = true);
        let error = server
            .host()
            .publish(&release, &commit, NOTES, &artifact)
            .unwrap_err();
        assert!(error.to_string().contains("fresh GitHub archive asset"));
        server.edit(|state| assert_eq!(state.mutations.len(), 2));
    }
}
