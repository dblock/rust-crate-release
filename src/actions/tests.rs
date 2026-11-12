use super::*;
use crate::actions::test_support::{Server, TestResult, zip_files};
use std::collections::BTreeMap;

#[test]
fn guards_require_manual_default_branch_hosted_oidc_context() -> Result {
    let valid: BTreeMap<String, String> = [
        ("GITHUB_ACTIONS", "true"),
        ("GITHUB_SERVER_URL", "https://github.com"),
        ("GITHUB_EVENT_NAME", "workflow_dispatch"),
        ("GITHUB_REF", "refs/heads/main"),
        ("RCR_DEFAULT_BRANCH", "main"),
        ("RUNNER_ENVIRONMENT", "github-hosted"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect();
    guard(&|key| valid.get(key).cloned())?;
    for (key, value) in [
        ("GITHUB_EVENT_NAME", "push"),
        ("GITHUB_REF", "refs/heads/topic"),
        ("RUNNER_ENVIRONMENT", "self-hosted"),
        ("CARGO_REGISTRY_TOKEN", "unexpected-stored-token"),
        ("GITHUB_SERVER_URL", "https://example.invalid"),
    ] {
        let mut invalid = valid.clone();
        invalid.insert(key.into(), value.into());
        assert!(guard(&|key| invalid.get(key).cloned()).is_err());
    }
    Ok(())
}

#[test]
fn downloaded_snapshot_must_match_exact_recovery_bytes_without_links_or_extra_files() -> TestResult
{
    let temporary = tempfile::tempdir()?;
    let files: Vec<(String, Vec<u8>)> = [
        ("example-0.1.0.crate", "exact crate bytes"),
        ("example-0.1.0.bundle", "recovery bundle"),
        ("source-inputs.json", "{}"),
        ("release.json", "{}"),
        ("execution.json", "{}"),
    ]
    .into_iter()
    .map(|(name, bytes)| (name.into(), bytes.as_bytes().into()))
    .collect();
    for (name, bytes) in &files {
        fs::write(temporary.path().join(name), bytes)?;
    }
    let check = |files: &[(String, Vec<u8>)]| -> TestResult {
        Ok(verify_snapshot(
            &zip_files(files)?,
            temporary.path(),
            "example-0.1.0",
        )?)
    };
    check(&files)?;
    for index in 0..3 {
        let mut missing = files.clone();
        missing.remove(index);
        assert!(check(&missing).is_err());
        let mut changed = files.clone();
        changed[index].1[0] ^= 1;
        assert!(check(&changed).is_err());
    }
    for unexpected in [".env", "../escape", "/absolute", "nested/release.json"] {
        let mut extra = files.clone();
        extra.push((unexpected.into(), b"private".to_vec()));
        assert!(check(&extra).is_err());
    }
    assert!(verify_snapshot(b"not zip", temporary.path(), "example-0.1.0").is_err());
    #[cfg(unix)]
    {
        let path = temporary.path().join("example-0.1.0.bundle");
        fs::remove_file(&path)?;
        std::os::unix::fs::symlink(temporary.path().join("release.json"), path)?;
        assert!(check(&files).is_err());
    }
    Ok(())
}

#[test]
fn artifact_fixture_waits_for_complete_request_headers() -> TestResult {
    let server = Server::new(b"remote exact bytes".to_vec())?;
    let mut stream =
        std::net::TcpStream::connect(server.endpoint.strip_prefix("http://").unwrap())?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(b"GET /repos/fixture/project/actions/artifacts/123 HTTP/1.1\r\n")?;
    std::thread::sleep(Duration::from_millis(50));
    stream.write_all(b"Host: localhost\r\nConnection: close\r\n\r\n")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    let (_, body) = response
        .split_once("\r\n\r\n")
        .ok_or("missing HTTP response body")?;
    assert_eq!(
        serde_json::from_str::<Value>(body)?,
        *server.metadata.lock().unwrap()
    );
    assert_eq!(
        server.requests.lock().unwrap().as_slice(),
        [
            "GET /repos/fixture/project/actions/artifacts/123 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        ]
    );
    Ok(())
}

#[test]
fn rest_download_uses_real_remote_bytes_without_forwarding_authentication() -> TestResult {
    let server = Server::new(b"remote exact bytes".to_vec())?;
    let preserver = GitHubArtifacts::for_test_server(123, &server.endpoint)?;
    let bytes = preserver.download(
        Url::parse(&format!(
            "{}/repos/fixture/project/actions/artifacts/123/zip",
            server.endpoint
        ))?,
        18,
    )?;
    assert_eq!(bytes, b"remote exact bytes");
    *server.bytes.lock().unwrap() = b"truncated".to_vec();
    assert!(
        preserver
            .download(
                Url::parse(&format!(
                    "{}/repos/fixture/project/actions/artifacts/123/zip",
                    server.endpoint
                ))?,
                18
            )
            .is_err()
    );
    let requests = server.requests.lock().unwrap();
    assert!(
        requests[0]
            .to_ascii_lowercase()
            .contains("authorization: bearer rcr-loopback-test-token")
    );
    assert!(!requests[1].to_ascii_lowercase().contains("authorization:"));
    drop(requests);
    let valid = server.metadata.lock().unwrap().clone();
    let metadata: StoredArtifact = serde_json::from_value(valid.clone())?;
    metadata.validate(123, "42", "1")?;
    for (field, value) in [
        ("id", json!(124)),
        ("name", json!("rcr-release-42-2")),
        ("expired", json!(true)),
        ("size_in_bytes", json!(0)),
        ("size_in_bytes", json!(ZIP_LIMIT + 1)),
        ("workflow_run", json!({"id":43})),
    ] {
        let mut wrong = valid.clone();
        wrong[field] = value;
        assert!(
            serde_json::from_value::<StoredArtifact>(wrong)?
                .validate(123, "42", "1")
                .is_err()
        );
    }
    assert!(
        preserver
            .download(Url::parse(&format!("{}/missing", server.endpoint))?, 18)
            .is_err()
    );
    for invalid in [
        "https://example.com",
        "http://localhost",
        "http://127.0.0.1/path",
        "http://127.0.0.1?token=x",
    ] {
        assert!(GitHubArtifacts::for_test_server(123, invalid).is_err());
    }
    assert!(GitHubArtifacts::new(0).is_err());
    Ok(())
}
