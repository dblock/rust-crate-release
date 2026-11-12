//! Exact-byte Cargo registry publication. Upload acceptance is not index visibility.

use crate::execution::ExecutionProvider;
use crate::package::{Artifact, checksum, validate_name};
use crate::{Error, Result};
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use reqwest::{StatusCode, Url};
use semver::Version;
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedCrate {
    pub name: String,
    pub version: Version,
    pub checksum: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishOutcome {
    Published,
    Reused,
}

pub trait PackagePublisher: Send + Sync {
    fn lookup(&self, name: &str, version: &Version) -> Result<Option<PublishedCrate>>;
    fn publish(
        &self,
        artifact: &Artifact,
        provider: &dyn ExecutionProvider,
    ) -> Result<PublishOutcome>;
    /// Observes authentication without exposing credentials. Custom publishers
    /// with a separate authentication boundary should override this method.
    fn publish_with_authentication(
        &self,
        artifact: &Artifact,
        provider: &dyn ExecutionProvider,
        before: &dyn Fn() -> Result,
        after: &dyn Fn() -> Result,
    ) -> Result<PublishOutcome> {
        before()?;
        after()?;
        self.publish(artifact, provider)
    }
    fn download(&self, name: &str, version: &Version) -> Result<Vec<u8>>;
}

/// No credential is read until a missing version actually needs publication.
pub struct CratesIoPublisher {
    client: Client,
    endpoint: Url,
    authentication: AuthenticationSource,
}

enum AuthenticationSource {
    Environment,
    Loopback(Option<SecretToken>),
}

struct SecretToken(String);

struct Authentication {
    token: SecretToken,
    revoke: bool,
}

impl CratesIoPublisher {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: client(false)?,
            endpoint: Url::parse("https://crates.io")
                .map_err(|_| Error::invalid("invalid built-in registry URL"))?,
            authentication: AuthenticationSource::Environment,
        })
    }

    /// An isolated simulator: never reads environment credentials or requests OIDC.
    ///
    /// Only literal loopback IP addresses with no URL path or credentials are allowed.
    pub fn with_endpoint(endpoint: &str, token: Option<String>) -> Result<Self> {
        let endpoint =
            Url::parse(endpoint).map_err(|_| Error::invalid("invalid local registry endpoint"))?;
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
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.path() != "/"
        {
            return Err(Error::invalid(
                "simulated registries require a bare loopback endpoint",
            ));
        }
        let token = token.map(SecretToken);
        if let Some(token) = &token {
            token.header()?;
        }
        Ok(Self {
            client: client(true)?,
            endpoint,
            authentication: AuthenticationSource::Loopback(token),
        })
    }

    fn url(&self, path: &str) -> Result<Url> {
        self.endpoint
            .join(path)
            .map_err(|_| Error::invalid("invalid registry API path"))
    }

    fn authenticate(&self, provider: &dyn ExecutionProvider) -> Result<Authentication> {
        match &self.authentication {
            AuthenticationSource::Loopback(token) => Ok(Authentication {
                token: SecretToken(
                    token
                        .as_ref()
                        .ok_or_else(|| {
                            Error::invalid(
                                "local registry publication requires an explicit test token",
                            )
                        })?
                        .0
                        .clone(),
                ),
                revoke: false,
            }),
            AuthenticationSource::Environment => {
                if let Some(token) = std::env::var("CARGO_REGISTRY_TOKEN")
                    .ok()
                    .filter(|value| !value.is_empty())
                {
                    let token = SecretToken(token);
                    token.header()?;
                    if provider.metadata().platform == "github" {
                        mask(&token.0);
                    }
                    return Ok(Authentication {
                        token,
                        revoke: false,
                    });
                }
                if provider.metadata().platform != "github" {
                    return Err(Error::invalid(
                        "crates.io publication requires CARGO_REGISTRY_TOKEN or GitHub trusted publishing",
                    ));
                }
                self.github_authentication()
            }
        }
    }

    fn github_authentication(&self) -> Result<Authentication> {
        let request_url = std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL")
            .ok()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::invalid("GitHub trusted publishing requires id-token: write"))?;
        let request_token = std::env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
            .ok()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::invalid("GitHub identity request token is unavailable"))?;
        let request_url = Url::parse(&request_url)
            .map_err(|_| Error::invalid("invalid GitHub identity request endpoint"))?;
        let host = request_url.host_str().unwrap_or_default();
        if request_url.scheme() != "https"
            || !(host == "actions.githubusercontent.com"
                || host.ends_with(".actions.githubusercontent.com"))
            || !request_url.username().is_empty()
            || request_url.password().is_some()
            || request_url.fragment().is_some()
        {
            return Err(Error::invalid("untrusted GitHub identity request endpoint"));
        }
        self.exchange_identity(request_url, &SecretToken(request_token))
    }

    fn exchange_identity(
        &self,
        mut request_url: Url,
        request_token: &SecretToken,
    ) -> Result<Authentication> {
        let query: Vec<_> = request_url
            .query_pairs()
            .filter(|(key, _)| key != "audience")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        request_url.set_query(None);
        request_url
            .query_pairs_mut()
            .extend_pairs(query)
            .append_pair("audience", "crates.io");
        mask(&request_token.0);
        let response = self
            .client
            .get(request_url)
            .bearer_auth(&request_token.0)
            .header(ACCEPT, "application/json")
            .send()
            .map_err(|_| Error::invalid("GitHub identity request failed"))?;
        let identity = authentication_json(response, "GitHub identity request")?;
        let jwt = identity["value"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::invalid("GitHub identity response contains no token"))?;
        mask(jwt);
        let response = self
            .client
            .post(self.url("/api/v1/trusted_publishing/tokens")?)
            .json(&json!({ "jwt": jwt }))
            .send()
            .map_err(|_| Error::invalid("trusted publishing token exchange failed"))?;
        let exchanged = authentication_json(response, "trusted publishing token exchange")?;
        let token = SecretToken(
            exchanged["token"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| Error::invalid("trusted publishing response contains no token"))?
                .into(),
        );
        mask(&token.0);
        // Header validation happens inside upload so even a malformed issued token is revoked.
        Ok(Authentication {
            token,
            revoke: true,
        })
    }

    fn revoke(&self, authentication: &Authentication) -> Result {
        if !authentication.revoke {
            return Ok(());
        }
        let response = self
            .client
            .delete(self.url("/api/v1/trusted_publishing/tokens")?)
            .bearer_auth(&authentication.token.0)
            .send()
            .map_err(|_| Error::invalid("trusted publishing token revocation failed"))?;
        if !response.status().is_success() {
            return Err(Error::invalid(
                "trusted publishing token revocation was rejected",
            ));
        }
        Ok(())
    }

    fn upload(&self, body: Vec<u8>, authentication: &Authentication) -> Result<PublishOutcome> {
        let response = self.client
            .put(self.url("/api/v1/crates/new")?)
            .header(AUTHORIZATION, authentication.token.header()?)
            .header(CONTENT_TYPE, "application/octet-stream")
            .header(ACCEPT, "application/json")
            .body(body)
            .send()
            .map_err(|error| {
                transport_error(error).context(
                    "publication outcome is unknown; reconcile the registry before any further upload",
                )
            })?;
        let status = response.status();
        let result: Value = response.json().map_err(|error| {
            transport_error(error).context(
                "publication response is invalid; reconcile the registry before any further upload",
            )
        })?;
        if !status.is_success()
            || !result.is_object()
            || result.get("errors").is_some()
            || result
                .get("ok")
                .is_some_and(|ok| ok.as_bool() != Some(true))
        {
            return Err(Error::invalid(format!(
                "registry publication was not confirmed (HTTP {}); reconcile before another upload",
                status.as_u16()
            )));
        }

        if let Some(warnings) = result.get("warnings") {
            let warnings = warnings.as_object().ok_or_else(|| {
                Error::invalid("invalid publication warnings; reconcile before another upload")
            })?;
            for name in ["invalid_categories", "invalid_badges", "other"] {
                if let Some(values) = warnings.get(name)
                    && !values
                        .as_array()
                        .is_some_and(|values| values.iter().all(Value::is_string))
                {
                    return Err(Error::invalid(
                        "invalid publication warnings; reconcile before another upload",
                    ));
                }
            }
        }
        Ok(PublishOutcome::Published)
    }

    fn upload_authenticated(
        &self,
        body: Vec<u8>,
        authentication: &Authentication,
        provider: &dyn ExecutionProvider,
    ) -> Result<PublishOutcome> {
        let result = self.upload(body, authentication);
        let cleanup = self.revoke(authentication);
        if cleanup.is_err() {
            let _ = provider.summary(
                "Warning: Short-lived crates.io token revocation failed. Publication must be reconciled before another upload.",
            );
        }
        // Cleanup failure cannot turn accepted publication into an upload retry.
        result
    }
}

impl PackagePublisher for CratesIoPublisher {
    fn lookup(&self, name: &str, version: &Version) -> Result<Option<PublishedCrate>> {
        validate_name(name)?;
        let response = self
            .client
            .get(self.url(&format!("/api/v1/crates/{name}/{version}"))?)
            .header(ACCEPT, "application/json")
            .header("Cache-Control", "no-cache")
            .send()
            .map_err(transport_error)?;
        let status = response.status();
        let result: Value = response.json().map_err(transport_error)?;
        if status == StatusCode::NOT_FOUND && structured_errors(&result) {
            return Ok(None);
        }
        if !status.is_success() || result.get("errors").is_some() {
            return Err(Error::invalid(format!(
                "registry lookup failed (HTTP {})",
                status.as_u16()
            )));
        }
        let entry = &result["version"];
        if entry["crate"].as_str() != Some(name)
            || entry["num"].as_str() != Some(version.to_string().as_str())
        {
            return Err(Error::invalid(
                "registry lookup returned a different crate identity",
            ));
        }
        let checksum = entry["checksum"]
            .as_str()
            .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| Error::invalid("registry lookup returned an invalid SHA-256 checksum"))?
            .to_ascii_lowercase();
        Ok(Some(PublishedCrate {
            name: name.into(),
            version: version.clone(),
            checksum,
        }))
    }

    fn publish(
        &self,
        artifact: &Artifact,
        provider: &dyn ExecutionProvider,
    ) -> Result<PublishOutcome> {
        self.publish_with_authentication(artifact, provider, &|| Ok(()), &|| Ok(()))
    }

    fn publish_with_authentication(
        &self,
        artifact: &Artifact,
        provider: &dyn ExecutionProvider,
        before: &dyn Fn() -> Result,
        after: &dyn Fn() -> Result,
    ) -> Result<PublishOutcome> {
        let bytes = artifact.validated_bytes()?;
        if let Some(published) = self.lookup(artifact.name(), artifact.version())? {
            if published.checksum != artifact.checksum() {
                return Err(Error::invalid("published crate has a conflicting checksum"));
            }
            return Ok(PublishOutcome::Reused);
        }
        let body = upload_body(artifact.upload_metadata(), &bytes)?;
        before()?;
        let authentication = self.authenticate(provider)?;
        if let Err(error) = after() {
            if self.revoke(&authentication).is_err() {
                let _ = provider.summary("Warning: Short-lived crates.io token revocation failed after an authentication hook failure.");
            }
            return Err(error);
        }
        self.upload_authenticated(body, &authentication, provider)
    }

    fn download(&self, name: &str, version: &Version) -> Result<Vec<u8>> {
        let published = self
            .lookup(name, version)?
            .ok_or_else(|| Error::invalid("cannot download an unpublished crate version"))?;
        let url = match &self.authentication {
            AuthenticationSource::Environment => Url::parse(&format!(
                "https://static.crates.io/crates/{name}/{name}-{version}.crate"
            ))
            .map_err(|_| Error::invalid("invalid public crate download path"))?,
            AuthenticationSource::Loopback(_) => {
                self.url(&format!("/api/v1/crates/{name}/{version}/download"))?
            }
        };
        let response = self.client.get(url).send().map_err(transport_error)?;
        if !response.status().is_success() {
            return Err(Error::invalid(format!(
                "crate download failed (HTTP {})",
                response.status().as_u16()
            )));
        }
        let bytes = response.bytes().map_err(transport_error)?.to_vec();
        if checksum(&bytes) != published.checksum {
            return Err(Error::invalid(
                "downloaded crate checksum disagrees with registry metadata",
            ));
        }
        Ok(bytes)
    }
}

impl SecretToken {
    fn header(&self) -> Result<HeaderValue> {
        if self.0.is_empty()
            || self
                .0
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        {
            return Err(Error::invalid(
                "registry authentication token has an invalid format",
            ));
        }
        let mut header = HeaderValue::from_bytes(self.0.as_bytes())
            .map_err(|_| Error::invalid("registry authentication token has an invalid format"))?;
        header.set_sensitive(true);
        Ok(header)
    }
}

fn client(loopback: bool) -> Result<Client> {
    let mut builder = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("rust-crate-release/", env!("CARGO_PKG_VERSION")));
    if loopback {
        builder = builder.no_proxy();
    }
    builder.build().map_err(transport_error)
}

fn transport_error(error: reqwest::Error) -> Error {
    Error::from(std::io::Error::other(error.without_url())).context("registry request failed")
}

fn structured_errors(value: &Value) -> bool {
    value["errors"].as_array().is_some_and(|errors| {
        !errors.is_empty()
            && errors.iter().all(|error| {
                error["detail"]
                    .as_str()
                    .is_some_and(|detail| !detail.is_empty())
            })
    })
}

fn authentication_json(response: Response, operation: &str) -> Result<Value> {
    if !response.status().is_success() {
        return Err(Error::invalid(format!("{operation} was rejected")));
    }
    let value: Value = response
        .json()
        .map_err(|_| Error::invalid(format!("{operation} returned invalid JSON")))?;
    if !value.is_object() || value.get("errors").is_some() {
        return Err(Error::invalid(format!("{operation} failed")));
    }
    Ok(value)
}

fn mask(secret: &str) {
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return;
    }
    let escaped = secret
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A");
    eprintln!("::add-mask::{escaped}");
}

fn upload_body(metadata: &Value, bytes: &[u8]) -> Result<Vec<u8>> {
    let metadata = serde_json::to_vec(metadata)?;
    let metadata_length = u32::try_from(metadata.len())
        .map_err(|_| Error::invalid("Cargo upload metadata exceeds the u32 protocol limit"))?;
    let archive_length = u32::try_from(bytes.len())
        .map_err(|_| Error::invalid("Cargo archive exceeds the u32 protocol limit"))?;
    let length = metadata
        .len()
        .checked_add(bytes.len())
        .and_then(|length| length.checked_add(8))
        .ok_or_else(|| Error::invalid("Cargo upload body length overflow"))?;
    let mut body = Vec::new();
    body.try_reserve_exact(length)
        .map_err(|_| Error::invalid("cannot allocate Cargo upload body"))?;
    body.extend_from_slice(&metadata_length.to_le_bytes());
    body.extend_from_slice(&metadata);
    body.extend_from_slice(&archive_length.to_le_bytes());
    body.extend_from_slice(bytes);
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::{LocalExecutionProvider, RunMetadata};
    use crate::package::tests::{metadata_fixture, retained_fixture};
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Mutex;
    use std::thread::{self, JoinHandle};
    use std::time::Instant;

    #[test]
    fn actions_masking_keeps_machine_stdout_clean() -> Result {
        if std::env::var("RCR_TEST_MASK_STDERR_CHILD").as_deref() == Ok("1") {
            mask("dummy%token\r\n");
            return Ok(());
        }
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "registry::tests::actions_masking_keeps_machine_stdout_clean",
                "--nocapture",
            ])
            .env("RCR_TEST_MASK_STDERR_CHILD", "1")
            .env("GITHUB_ACTIONS", "true")
            .output()?;
        assert!(output.status.success());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("::add-mask::"));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("::add-mask::dummy%25token%0D%0A")
        );
        Ok(())
    }

    struct Request {
        method: String,
        path: String,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    }

    enum Reply {
        Json(u16, Value),
        Bytes(u16, Vec<u8>),
        Redirect,
        Disconnect,
    }

    struct Server {
        endpoint: String,
        worker: JoinHandle<Vec<Request>>,
    }

    impl Server {
        fn start(replies: Vec<Reply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let worker = thread::spawn(move || {
                let mut requests = Vec::new();
                for reply in replies {
                    let deadline = Instant::now() + Duration::from_secs(10);
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                assert!(Instant::now() < deadline, "registry simulator timed out");
                                thread::sleep(Duration::from_millis(5));
                            }
                            Err(error) => panic!("registry simulator accept failed: {error}"),
                        }
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    requests.push(read_request(&mut stream));
                    match reply {
                        Reply::Disconnect => continue,
                        Reply::Redirect => {
                            stream.write_all(
                                b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            ).unwrap();
                        }
                        reply => {
                            let (status, body) = match reply {
                                Reply::Json(status, body) => {
                                    (status, serde_json::to_vec(&body).unwrap())
                                }
                                Reply::Bytes(status, body) => (status, body),
                                _ => unreachable!(),
                            };
                            write!(stream, "HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                            stream.write_all(&body).unwrap();
                        }
                    }
                }
                // A disconnected upload must not cause another request, even automatically.
                let deadline = Instant::now() + Duration::from_millis(100);
                while Instant::now() < deadline {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream.set_nonblocking(false).unwrap();
                            stream
                                .set_read_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            stream
                                .set_write_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            requests.push(read_request(&mut stream));
                            let _ = stream.write_all(
                                b"HTTP/1.1 500 Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            );
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("registry simulator accept failed: {error}"),
                    }
                }
                requests
            });
            Self { endpoint, worker }
        }

        fn publisher(&self) -> CratesIoPublisher {
            CratesIoPublisher::with_endpoint(&self.endpoint, Some("isolated-test-token".into()))
                .unwrap()
        }

        fn finish(self) -> Vec<Request> {
            self.worker.join().unwrap()
        }
    }

    fn read_request(stream: &mut TcpStream) -> Request {
        let mut bytes = Vec::new();
        let header_end = loop {
            if let Some(position) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break position + 4;
            }
            let mut chunk = [0; 4096];
            let read = stream.read(&mut chunk).unwrap();
            assert_ne!(read, 0, "unexpected end of HTTP request");
            bytes.extend_from_slice(&chunk[..read]);
        };
        let head = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
        let mut lines = head.split("\r\n");
        let mut first = lines.next().unwrap().split_whitespace();
        let method = first.next().unwrap().into();
        let path = first.next().unwrap().into();
        let headers: BTreeMap<_, _> = lines
            .filter_map(|line| {
                line.split_once(':')
                    .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().into()))
            })
            .collect();
        let length: usize = headers
            .get("content-length")
            .map_or(0, |length: &String| length.parse().unwrap());
        while bytes.len() < header_end + length {
            let mut chunk = [0; 4096];
            let read = stream.read(&mut chunk).unwrap();
            assert_ne!(read, 0, "unexpected end of HTTP body");
            bytes.extend_from_slice(&chunk[..read]);
        }
        Request {
            method,
            path,
            headers,
            body: bytes[header_end..header_end + length].to_vec(),
        }
    }

    fn missing() -> Reply {
        Reply::Json(
            404,
            json!({"errors": [{"detail": "crate version does not exist"}]}),
        )
    }

    fn existing(artifact: &Artifact) -> Reply {
        Reply::Json(
            200,
            json!({
                "version": { "crate": artifact.name(), "num": artifact.version().to_string(), "checksum": artifact.checksum() }
            }),
        )
    }

    #[test]
    fn authentication_callbacks_precede_upload_and_are_skipped_for_existing_versions() {
        let (_fixture, artifact) = retained_fixture();
        let server = Server::start(vec![missing()]);
        let before = std::cell::Cell::new(false);
        let error = server
            .publisher()
            .publish_with_authentication(
                &artifact,
                &LocalExecutionProvider,
                &|| {
                    before.set(true);
                    Ok(())
                },
                &|| {
                    assert!(before.get());
                    Err(Error::invalid("authentication callback stopped upload"))
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("callback stopped"));
        assert_eq!(server.finish().len(), 1);
        let server = Server::start(vec![existing(&artifact)]);
        assert_eq!(
            server
                .publisher()
                .publish_with_authentication(
                    &artifact,
                    &LocalExecutionProvider,
                    &|| panic!("existing versions must not authenticate"),
                    &|| panic!("existing versions must not authenticate"),
                )
                .unwrap(),
            PublishOutcome::Reused
        );
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn uploads_exact_retained_bytes_and_normalized_dependency_metadata() {
        let (_fixture, artifact) = metadata_fixture();
        let artifact = &artifact;
        let server = Server::start(vec![
            missing(),
            Reply::Json(
                200,
                json!({"warnings": {"invalid_categories": [], "invalid_badges": [], "other": ["test warning"]}}),
            ),
        ]);
        assert_eq!(
            server
                .publisher()
                .publish(artifact, &LocalExecutionProvider)
                .unwrap(),
            PublishOutcome::Published
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert!(!requests[0].headers.contains_key("authorization"));
        let upload = &requests[1];
        assert_eq!(upload.method, "PUT");
        assert_eq!(upload.path, "/api/v1/crates/new");
        assert_eq!(upload.headers["authorization"], "isolated-test-token");
        assert_eq!(upload.headers["content-type"], "application/octet-stream");
        assert_eq!(upload.headers["accept"], "application/json");
        let metadata_length = u32::from_le_bytes(upload.body[..4].try_into().unwrap()) as usize;
        let metadata: Value = serde_json::from_slice(&upload.body[4..4 + metadata_length]).unwrap();
        assert_eq!(&metadata, artifact.upload_metadata());
        let archive_offset = 4 + metadata_length;
        let archive_length = u32::from_le_bytes(
            upload.body[archive_offset..archive_offset + 4]
                .try_into()
                .unwrap(),
        ) as usize;
        let bytes = &upload.body[archive_offset + 4..];
        assert_eq!(bytes.len(), archive_length);
        assert_eq!(bytes, std::fs::read(artifact.path()).unwrap());
        assert_eq!(checksum(bytes), artifact.checksum());
        assert!(metadata["deps"].as_array().unwrap().iter().any(|dep| {
            dep["name"] == "serde"
                && dep["explicit_name_in_toml"] == "renamed"
                && dep["optional"] == true
                && dep["default_features"] == false
                && dep["features"] == json!(["derive"])
        }));
        assert!(
            metadata["deps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|dep| dep["kind"] == "dev")
        );
        assert!(
            metadata["deps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|dep| dep["kind"] == "build")
        );
        assert!(
            metadata["deps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|dep| dep["target"] == "cfg(windows)")
        );
    }

    #[test]
    fn existing_matching_versions_reuse_without_authentication_and_conflicts_fail() {
        let (_fixture, artifact) = retained_fixture();
        let artifact = &artifact;
        let server = Server::start(vec![existing(artifact)]);
        let publisher = CratesIoPublisher::with_endpoint(&server.endpoint, None).unwrap();
        assert_eq!(
            publisher
                .publish(artifact, &LocalExecutionProvider)
                .unwrap(),
            PublishOutcome::Reused
        );
        assert_eq!(server.finish().len(), 1);
        let server = Server::start(vec![Reply::Json(
            200,
            json!({
                "version": {"crate": artifact.name(), "num": artifact.version().to_string(), "checksum": "0".repeat(64)}
            }),
        )]);
        assert!(
            server
                .publisher()
                .publish(artifact, &LocalExecutionProvider)
                .unwrap_err()
                .to_string()
                .contains("conflicting")
        );
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn ambiguous_upload_is_never_retried_and_fresh_reconciliation_reuses_acceptance() {
        let (_fixture, artifact) = retained_fixture();
        let artifact = &artifact;
        let server = Server::start(vec![missing(), Reply::Disconnect]);
        let error = server
            .publisher()
            .publish(artifact, &LocalExecutionProvider)
            .unwrap_err();
        assert!(error.to_string().contains("unknown"));
        assert!(!error.to_string().contains("isolated-test-token"));
        assert!(!error.to_string().contains(&server.endpoint));
        assert_eq!(server.finish().len(), 2);
        let server = Server::start(vec![existing(artifact)]);
        assert_eq!(
            server
                .publisher()
                .publish(artifact, &LocalExecutionProvider)
                .unwrap(),
            PublishOutcome::Reused
        );
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn only_structured_missing_versions_are_absent() {
        let (_fixture, artifact) = retained_fixture();
        let artifact = &artifact;
        let replies = vec![
            Reply::Bytes(404, b"not found".to_vec()),
            Reply::Json(404, json!({"errors": []})),
            Reply::Json(404, json!({"errors": [{"unexpected": "field"}]})),
            Reply::Json(
                403,
                json!({"errors": [{"detail": "secret-authentication-response"}]}),
            ),
            Reply::Json(
                200,
                json!({"errors": [{"detail": "secret-authentication-response"}]}),
            ),
            Reply::Json(
                200,
                json!({"version": {"crate": "different", "num": artifact.version().to_string(), "checksum": artifact.checksum()}}),
            ),
            Reply::Json(
                200,
                json!({"version": {"crate": artifact.name(), "num": "9.0.0", "checksum": artifact.checksum()}}),
            ),
            Reply::Json(
                200,
                json!({"version": {"crate": artifact.name(), "num": artifact.version().to_string(), "checksum": "not-a-checksum"}}),
            ),
            Reply::Redirect,
        ];
        let count = replies.len();
        let server = Server::start(replies);
        let publisher = server.publisher();
        for _ in 0..count {
            let error = publisher
                .lookup(artifact.name(), artifact.version())
                .unwrap_err()
                .to_string();
            assert!(!error.contains("secret-authentication-response"));
            assert!(!error.contains("isolated-test-token"));
        }
        assert_eq!(server.finish().len(), count);
        let server = Server::start(vec![missing()]);
        assert!(
            server
                .publisher()
                .lookup(artifact.name(), artifact.version())
                .unwrap()
                .is_none()
        );
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn downloads_public_bytes_without_credentials_and_verifies_checksums() {
        let (_fixture, artifact) = retained_fixture();
        let artifact = &artifact;
        let bytes = std::fs::read(artifact.path()).unwrap();
        let server = Server::start(vec![existing(artifact), Reply::Bytes(200, bytes.clone())]);
        assert_eq!(
            server
                .publisher()
                .download(artifact.name(), artifact.version())
                .unwrap(),
            bytes
        );
        let requests = server.finish();
        assert_eq!(
            requests[1].path,
            format!(
                "/api/v1/crates/{}/{}/download",
                artifact.name(),
                artifact.version()
            )
        );
        assert!(
            requests
                .iter()
                .all(|request| !request.headers.contains_key("authorization"))
        );
        let server = Server::start(vec![
            existing(artifact),
            Reply::Bytes(200, b"corrupted".to_vec()),
        ]);
        assert!(
            server
                .publisher()
                .download(artifact.name(), artifact.version())
                .is_err()
        );
        assert_eq!(server.finish().len(), 2);
        let server = Server::start(vec![existing(artifact), Reply::Redirect]);
        assert!(
            server
                .publisher()
                .download(artifact.name(), artifact.version())
                .is_err()
        );
        assert_eq!(server.finish().len(), 2);
    }

    #[test]
    fn invalid_publish_responses_and_warnings_never_authorize_another_upload() {
        let (_fixture, artifact) = retained_fixture();
        let artifact = &artifact;
        for reply in [
            Reply::Json(
                403,
                json!({"errors": [{"detail": "must-not-leak-response-secret"}]}),
            ),
            Reply::Json(
                200,
                json!({"errors": [{"detail": "must-not-leak-response-secret"}]}),
            ),
            Reply::Json(200, json!({"ok": false})),
            Reply::Json(200, json!({"warnings": "wrong"})),
            Reply::Json(200, json!({"warnings": {"other": [42]}})),
            Reply::Bytes(200, b"invalid JSON must-not-leak-response-secret".to_vec()),
            Reply::Redirect,
        ] {
            let server = Server::start(vec![missing(), reply]);
            let error = server
                .publisher()
                .publish(artifact, &LocalExecutionProvider)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("must-not-leak-response-secret"));
            assert!(!error.contains("isolated-test-token"));
            assert_eq!(server.finish().len(), 2);
        }
    }

    #[test]
    fn loopback_authentication_never_falls_back_to_environment_or_oidc() {
        let (_fixture, artifact) = retained_fixture();
        let artifact = &artifact;
        let server = Server::start(vec![missing()]);
        let publisher = CratesIoPublisher::with_endpoint(&server.endpoint, None).unwrap();
        let error = publisher
            .publish(artifact, &RecordingProvider::default())
            .unwrap_err();
        assert!(error.to_string().contains("explicit test token"));
        assert_eq!(server.finish().len(), 1);
        for endpoint in [
            "https://crates.io",
            "http://localhost:8000",
            "http://127.0.0.1/api",
            "http://user:password@127.0.0.1",
            "http://127.0.0.1?token=secret",
            "http://127.0.0.1#secret",
        ] {
            assert!(CratesIoPublisher::with_endpoint(endpoint, Some("secret".into())).is_err());
        }
        let error =
            CratesIoPublisher::with_endpoint("http://127.0.0.1", Some("secret\ninvalid".into()))
                .err()
                .unwrap()
                .to_string();
        assert!(!error.contains("secret"));
        assert!(SecretToken(String::new()).header().is_err());
    }

    #[derive(Default)]
    struct RecordingProvider {
        summaries: Mutex<Vec<String>>,
    }

    impl ExecutionProvider for RecordingProvider {
        fn metadata(&self) -> RunMetadata {
            RunMetadata {
                platform: "github".into(),
                ..RunMetadata::default()
            }
        }

        fn summary(&self, markdown: &str) -> Result {
            self.summaries.lock().unwrap().push(markdown.into());
            Err(Error::invalid("simulated summary failure"))
        }
    }

    #[test]
    fn simulated_oidc_exchange_scopes_audience_and_revokes_after_publication() {
        let server = Server::start(vec![
            Reply::Json(200, json!({"value": "explicit-test-jwt"})),
            Reply::Json(200, json!({"token": "explicit-short-lived-token"})),
            Reply::Json(200, json!({"warnings": {}})),
            Reply::Json(200, json!({})),
        ]);
        let publisher = server.publisher();
        let authentication = publisher
            .exchange_identity(
                Url::parse(&format!(
                    "{}/identity?audience=wrong&request=retained",
                    server.endpoint
                ))
                .unwrap(),
                &SecretToken("explicit-request-token".into()),
            )
            .unwrap();
        assert_eq!(
            publisher
                .upload_authenticated(vec![1, 2, 3], &authentication, &LocalExecutionProvider)
                .unwrap(),
            PublishOutcome::Published
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 4);
        assert!(requests[0].path.contains("audience=crates.io"));
        assert!(!requests[0].path.contains("audience=wrong"));
        assert!(requests[0].path.contains("request=retained"));
        assert_eq!(
            requests[0].headers["authorization"],
            "Bearer explicit-request-token"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&requests[1].body).unwrap(),
            json!({"jwt": "explicit-test-jwt"})
        );
        assert_eq!(requests[1].method, "POST");
        assert_eq!(requests[1].path, "/api/v1/trusted_publishing/tokens");
        assert_eq!(
            requests[2].headers["authorization"],
            "explicit-short-lived-token"
        );
        assert_eq!(requests[3].method, "DELETE");
        assert_eq!(requests[3].path, "/api/v1/trusted_publishing/tokens");
        assert_eq!(
            requests[3].headers["authorization"],
            "Bearer explicit-short-lived-token"
        );
    }

    #[test]
    fn cleanup_preserves_upload_error_and_cannot_turn_acceptance_into_failure() {
        for accepted in [false, true] {
            let server = Server::start(vec![
                Reply::Json(
                    if accepted { 200 } else { 403 },
                    if accepted {
                        json!({})
                    } else {
                        json!({"errors": [{"detail": "secret"}]})
                    },
                ),
                Reply::Json(500, json!({"errors": [{"detail": "revocation-secret"}]})),
            ]);
            let publisher = server.publisher();
            let provider = RecordingProvider::default();
            let result = publisher.upload_authenticated(
                vec![],
                &Authentication {
                    token: SecretToken("short-lived-test-token".into()),
                    revoke: true,
                },
                &provider,
            );
            if accepted {
                assert_eq!(result.unwrap(), PublishOutcome::Published);
            } else {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("HTTP 403"));
                assert!(!error.contains("secret"));
                assert!(!error.contains("revocation"));
            }
            assert_eq!(provider.summaries.lock().unwrap().len(), 1);
            let requests = server.finish();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[1].method, "DELETE");
        }
    }

    #[test]
    fn authentication_response_errors_never_expose_server_payloads() {
        for reply in [
            Reply::Json(
                403,
                json!({"errors": [{"detail": "secret-authentication-payload"}]}),
            ),
            Reply::Bytes(200, b"secret-authentication-payload".to_vec()),
            Reply::Json(
                200,
                json!({"errors": [{"detail": "secret-authentication-payload"}]}),
            ),
        ] {
            let server = Server::start(vec![reply]);
            let publisher = server.publisher();
            let response = publisher
                .client
                .get(publisher.url("/authentication").unwrap())
                .send()
                .unwrap();
            let error = authentication_json(response, "simulated authentication")
                .unwrap_err()
                .to_string();
            assert!(!error.contains("secret-authentication-payload"));
            assert_eq!(server.finish().len(), 1);
        }
    }
}
