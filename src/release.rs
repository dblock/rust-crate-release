use crate::{Error, Result};
use semver::Version;
use serde::Serialize;

/// Captured release identity; development metadata cannot change these values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Release {
    name: String,
    version: Version,
    manifest_version: Version,
    source_commit: String,
    date: String,
    tag: String,
}

impl Release {
    pub fn new(
        name: impl Into<String>,
        version: Version,
        manifest_version: Version,
        source_commit: impl Into<String>,
        date: impl Into<String>,
    ) -> Result<Self> {
        let name = name.into();
        let source_commit = source_commit.into();
        let date = date.into();
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(Error::invalid("release name must be a Cargo package name"));
        }
        if !version.pre.is_empty() || !version.build.is_empty() {
            return Err(Error::invalid(
                "prerelease and build-metadata versions are unsupported",
            ));
        }
        if !matches!(source_commit.len(), 40 | 64)
            || !source_commit.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(Error::invalid(
                "release source must be a complete Git object ID",
            ));
        }
        if date.len() != 10
            || date.as_bytes()[4] != b'/'
            || date.as_bytes()[7] != b'/'
            || !date
                .bytes()
                .enumerate()
                .all(|(i, c)| matches!(i, 4 | 7) || c.is_ascii_digit())
        {
            return Err(Error::invalid("release date must use YYYY/MM/DD"));
        }
        let year: u16 = date[..4].parse().unwrap();
        let month: u8 = date[5..7].parse().unwrap();
        let day: u8 = date[8..].parse().unwrap();
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let last_day = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => 0,
        };
        if year == 0 || day == 0 || day > last_day {
            return Err(Error::invalid("release date is not a valid calendar date"));
        }
        let tag = format!("v{version}");
        Ok(Self {
            name,
            version,
            manifest_version,
            source_commit,
            date,
            tag,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn version(&self) -> &Version {
        &self.version
    }
    pub fn manifest_version(&self) -> &Version {
        &self.manifest_version
    }
    pub fn source_commit(&self) -> &str {
        &self.source_commit
    }
    pub fn date(&self) -> &str {
        &self.date
    }
    pub fn tag(&self) -> &str {
        &self.tag
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_identity_fails_before_any_operation() {
        for (name, version, source, date) in [
            ("../crate", "1.0.0", "a".repeat(40), "2026/11/12"),
            ("crate", "1.0.0-alpha", "a".repeat(40), "2026/11/12"),
            ("crate", "1.0.0", "HEAD".into(), "2026/11/12"),
            ("crate", "1.0.0", "a".repeat(40), "2026-11-12"),
            ("crate", "1.0.0", "a".repeat(40), "2026/02/29"),
            ("crate", "1.0.0", "a".repeat(40), "2026/13/01"),
            ("crate", "1.0.0", "a".repeat(40), "2026/01/00"),
        ] {
            assert!(
                Release::new(
                    name,
                    Version::parse(version).unwrap(),
                    Version::new(0, 1, 0),
                    source,
                    date
                )
                .is_err()
            );
        }
        assert!(
            Release::new(
                "crate",
                Version::new(1, 0, 0),
                Version::new(0, 1, 0),
                "a".repeat(40),
                "2028/02/29"
            )
            .is_ok()
        );
    }
}
