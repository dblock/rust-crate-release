use reqwest::Url;

pub(crate) fn is_loopback_origin(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && url
            .host_str()
            .and_then(|host| {
                host.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .ok()
            })
            .is_some_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use crate::{
        actions::GitHubArtifacts, hosting::GitHubReleaseHost, registry::CratesIoPublisher,
    };

    fn providers_accept(endpoint: &str) -> [bool; 3] {
        [
            GitHubArtifacts::for_test_server(1, endpoint).is_ok(),
            GitHubReleaseHost::for_test_server("owner/repository", endpoint).is_ok(),
            CratesIoPublisher::with_endpoint(endpoint, None).is_ok(),
        ]
    }

    #[test]
    fn simulators_accept_bare_ipv4_and_ipv6_loopback_origins() {
        for endpoint in [
            "http://127.0.0.1",
            "http://127.0.0.2:8080/",
            "https://127.0.0.1:443/",
            "http://[::1]:8080/",
            "https://[::1]/",
        ] {
            assert_eq!(providers_accept(endpoint), [true; 3], "{endpoint}");
        }
    }

    #[test]
    fn simulators_reject_non_loopback_hosts_and_non_origin_components() {
        for endpoint in [
            "not a URL",
            "http://localhost",
            "https://example.com",
            "http://192.0.2.1",
            "http://[2001:db8::1]",
            "http://[::ffff:127.0.0.1]",
            "file:///127.0.0.1",
            "ftp://127.0.0.1",
            "http://user@127.0.0.1",
            "http://:password@[::1]",
            "http://127.0.0.1/path",
            "http://127.0.0.1?",
            "http://[::1]#",
        ] {
            assert_eq!(providers_accept(endpoint), [false; 3], "{endpoint}");
        }
    }
}
