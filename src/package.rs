//! Cargo projects and immutable, inspected Cargo archives.

use crate::{Error, Result};
use flate2::bufread::GzDecoder;
use semver::Version;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use tar::Archive;
use toml_edit::{DocumentMut, Item, value};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageMetadata {
    pub name: String,
    pub version: Version,
    pub manifest_path: PathBuf,
}

pub trait PackageProject: Send + Sync {
    fn root(&self) -> &Path;
    fn metadata(&self) -> Result<PackageMetadata>;
    fn set_version(&self, version: &Version) -> Result;
    fn build(&self, directory: &Path, source_date_epoch: i64) -> Result<Artifact>;
    fn verify_consumer(&self, artifact: &Artifact) -> Result;
}

/// Identity and upload metadata are derived from the archive, never a receipt.
#[derive(Clone, Debug)]
pub struct Artifact {
    path: PathBuf,
    name: String,
    version: Version,
    checksum: String,
    upload_metadata: Value,
}

impl Artifact {
    pub fn open(path: &Path) -> Result<Self> {
        if path.extension().and_then(|extension| extension.to_str()) != Some("crate") {
            return Err(Error::invalid(
                "a Cargo artifact must have a .crate extension",
            ));
        }
        let path = dunce::canonicalize(path)?;
        let bytes = fs::read(&path)?;
        let inspected = inspect(&bytes)?;
        Ok(Self {
            path,
            name: inspected.name,
            version: inspected.version,
            checksum: checksum(&bytes),
            upload_metadata: inspected.upload_metadata,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &Version {
        &self.version
    }

    pub fn checksum(&self) -> &str {
        &self.checksum
    }

    pub fn upload_metadata(&self) -> &Value {
        &self.upload_metadata
    }

    pub fn verify(&self) -> Result {
        self.validated_bytes().map(|_| ())
    }

    /// Reads the clean source commit from the verified retained archive, if recorded.
    pub fn vcs_commit(&self) -> Result<Option<String>> {
        let bytes = self.validated_bytes()?;
        let extracted = extract(&bytes)?;
        let info = match fs::read(extracted.root.join(".cargo_vcs_info.json")) {
            Ok(info) => info,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(Error::from(error).context("reading Cargo archive VCS metadata"));
            }
        };
        let info: Value = serde_json::from_slice(&info)
            .map_err(|error| Error::from(error).context("invalid Cargo archive VCS metadata"))?;
        let git = info
            .get("git")
            .and_then(Value::as_object)
            .ok_or_else(|| Error::invalid("Cargo archive VCS metadata has no valid git object"))?;
        match git.get("dirty") {
            None | Some(Value::Bool(false)) => {}
            Some(Value::Bool(true)) => {
                return Err(Error::invalid("Cargo archive records dirty source"));
            }
            _ => {
                return Err(Error::invalid(
                    "Cargo archive VCS dirty field must be a boolean",
                ));
            }
        }
        let commit = git
            .get("sha1")
            .and_then(Value::as_str)
            .filter(|commit| {
                matches!(commit.len(), 40 | 64)
                    && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or_else(|| {
                Error::invalid(
                    "Cargo archive VCS commit must be a complete 40- or 64-digit hexadecimal hash",
                )
            })?;
        Ok(Some(commit.to_ascii_lowercase()))
    }

    /// Reads once so the publisher cannot upload different bytes after verification.
    pub(crate) fn validated_bytes(&self) -> Result<Vec<u8>> {
        let bytes = fs::read(&self.path)?;
        if checksum(&bytes) != self.checksum {
            return Err(Error::invalid("retained Cargo archive checksum changed"));
        }
        let inspected = inspect(&bytes)?;
        if inspected.name != self.name
            || inspected.version != self.version
            || inspected.upload_metadata != self.upload_metadata
        {
            return Err(Error::invalid("retained Cargo archive identity changed"));
        }
        Ok(bytes)
    }
}

#[derive(Clone, Debug)]
pub struct CargoPackage {
    root: PathBuf,
    manifest: PathBuf,
}

impl CargoPackage {
    /// Selects the root package, not the default members of a virtual workspace.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        Self::from_manifest(root.into().join("Cargo.toml"))
    }

    /// Explicitly selects one package, including a member of a workspace.
    pub fn from_manifest(manifest: impl Into<PathBuf>) -> Result<Self> {
        let manifest = dunce::canonicalize(manifest.into())?;
        if manifest.file_name().and_then(|name| name.to_str()) != Some("Cargo.toml") {
            return Err(Error::invalid("the package manifest must be Cargo.toml"));
        }
        let document = parse_document(&fs::read_to_string(&manifest)?)?;
        if document
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(Item::as_str)
            .is_none()
        {
            return Err(Error::invalid(
                "select an explicit package; virtual workspaces are ambiguous",
            ));
        }
        let root = manifest
            .parent()
            .ok_or_else(|| Error::invalid("package manifest has no parent"))?
            .to_path_buf();
        Ok(Self { root, manifest })
    }

    fn cargo_metadata(&self) -> Result<(Value, Value)> {
        let all = cargo_metadata(&self.manifest)?;
        let package = selected_package(&all, &self.manifest)?.clone();
        validate_publish(&package)?;
        Ok((all, package))
    }
}

impl PackageProject for CargoPackage {
    fn root(&self) -> &Path {
        &self.root
    }

    fn metadata(&self) -> Result<PackageMetadata> {
        let (_, package) = self.cargo_metadata()?;
        Ok(PackageMetadata {
            name: required_string(&package, "name")?.into(),
            version: Version::parse(required_string(&package, "version")?)?,
            manifest_path: self.manifest.clone(),
        })
    }

    fn set_version(&self, version: &Version) -> Result {
        let source = fs::read_to_string(&self.manifest)?;
        let mut document = parse_document(&source)?;
        let package = document
            .get_mut("package")
            .ok_or_else(|| Error::invalid("missing package manifest section"))?;
        let name = package
            .get("name")
            .and_then(Item::as_str)
            .ok_or_else(|| Error::invalid("missing package name"))?
            .to_owned();
        let version_item = package
            .get_mut("version")
            .ok_or_else(|| Error::invalid("package must have an explicit local version"))?;
        let previous = version_item
            .as_str()
            .ok_or_else(|| {
                Error::invalid("workspace-inherited versions cannot be edited independently")
            })?
            .to_owned();
        Version::parse(&previous)?;
        let (all, _) = self.cargo_metadata()?;
        let lock_path = Path::new(required_string(&all, "workspace_root")?).join("Cargo.lock");
        let lock_source = fs::read_to_string(&lock_path).map_err(|error| {
            Error::from(error).context("Cargo.lock is required; generate and review it explicitly")
        })?;
        let mut lock = parse_document(&lock_source)?;
        let packages = lock
            .get_mut("package")
            .and_then(Item::as_array_of_tables_mut)
            .ok_or_else(|| Error::invalid("Cargo.lock has no package entries"))?;
        let matches: Vec<_> = packages
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.get("name").and_then(Item::as_str) == Some(name.as_str())
                    && entry.get("version").and_then(Item::as_str) == Some(previous.as_str())
                    && entry.get("source").is_none()
            })
            .map(|(index, _)| index)
            .collect();
        if matches.len() != 1 {
            return Err(Error::invalid(
                "Cargo.lock must identify exactly one matching local package",
            ));
        }
        if previous == version.to_string() {
            return Ok(());
        }
        replace_string(version_item, &version.to_string())?;
        replace_string(
            &mut packages.get_mut(matches[0]).unwrap()["version"],
            &version.to_string(),
        )?;
        let old_reference = format!("{name} {previous}");
        let new_reference = format!("{name} {version}");
        for package in packages.iter_mut() {
            if let Some(dependencies) = package.get_mut("dependencies").and_then(Item::as_array_mut)
            {
                for dependency in dependencies.iter_mut() {
                    if dependency.as_str() == Some(old_reference.as_str()) {
                        let decor = dependency.decor().clone();
                        *dependency = toml_edit::Value::from(new_reference.as_str());
                        *dependency.decor_mut() = decor;
                    }
                }
            }
        }
        let manifest_output = render_document(&document, &source);
        let lock_output = render_document(&lock, &lock_source);
        fs::write(&self.manifest, manifest_output)?;
        fs::write(lock_path, lock_output)?;
        Ok(())
    }

    fn build(&self, directory: &Path, source_date_epoch: i64) -> Result<Artifact> {
        if source_date_epoch < 0 {
            return Err(Error::invalid("SOURCE_DATE_EPOCH cannot be negative"));
        }
        let (all, package) = self.cargo_metadata()?;
        let lock = Path::new(required_string(&all, "workspace_root")?).join("Cargo.lock");
        if !lock.is_file() {
            return Err(Error::invalid(
                "Cargo.lock is required; generate and review it before packaging",
            ));
        }
        let name = required_string(&package, "name")?;
        let version = Version::parse(required_string(&package, "version")?)?;
        fs::create_dir_all(directory)?;
        let directory = dunce::canonicalize(directory)?;
        let staging = tempfile::Builder::new()
            .prefix(".rcr-package-")
            .tempdir_in(&directory)?;
        let mut command = Command::new("cargo");
        command
            .current_dir(&self.root)
            .args([
                "package",
                "--locked",
                "--offline",
                "--allow-dirty",
                "--manifest-path",
            ])
            .arg(&self.manifest)
            .args(["--package", name, "--target-dir"])
            .arg(staging.path())
            .env("SOURCE_DATE_EPOCH", source_date_epoch.to_string());
        run_cargo(&mut command, "Cargo archive creation and compilation")?;
        let filename = format!("{name}-{version}.crate");
        let generated = staging.path().join("package").join(&filename);
        let artifact = Artifact::open(&generated)?;
        if artifact.name() != name || artifact.version() != &version {
            return Err(Error::invalid(
                "Cargo generated an unexpected package identity",
            ));
        }
        let bytes = artifact.validated_bytes()?;
        let destination = directory.join(filename);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
        {
            Ok(mut file) => {
                file.write_all(&bytes)?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if fs::symlink_metadata(&destination)?.file_type().is_symlink()
                    || fs::read(&destination)? != bytes
                {
                    return Err(Error::invalid(
                        "retained Cargo archive already exists with different bytes",
                    ));
                }
            }
            Err(error) => return Err(error.into()),
        }
        Artifact::open(&destination)
    }

    fn verify_consumer(&self, artifact: &Artifact) -> Result {
        let bytes = artifact.validated_bytes()?;
        let extracted = extract(&bytes)?;
        let manifest = extracted.root.join("Cargo.toml");
        let all = cargo_metadata(&manifest)?;
        let package = selected_package(&all, &manifest)?;
        let targets = package["targets"]
            .as_array()
            .ok_or_else(|| Error::invalid("archive contains no Cargo targets"))?;
        let mut has_library = false;
        let mut has_binary = false;
        let mut rust_library = false;
        for target in targets {
            let kinds = target["kind"]
                .as_array()
                .ok_or_else(|| Error::invalid("invalid Cargo target kinds"))?;
            has_library |= kinds.iter().any(|kind| {
                matches!(
                    kind.as_str(),
                    Some("lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro")
                )
            });
            has_binary |= kinds.iter().any(|kind| kind == "bin");
            rust_library |= kinds
                .iter()
                .any(|kind| matches!(kind.as_str(), Some("lib" | "rlib" | "dylib" | "proc-macro")));
        }
        if !has_library && !has_binary {
            return Err(Error::invalid(
                "archive has no library or binary consumer target",
            ));
        }
        let mut command = Command::new("cargo");
        command
            .current_dir(&extracted.root)
            .args(["build", "--locked", "--offline", "--manifest-path"])
            .arg(&manifest)
            .arg("--target-dir")
            .arg(extracted.directory.path().join("compiled"));
        if has_library {
            command.arg("--lib");
        }
        if has_binary {
            command.arg("--bins");
        }
        run_cargo(&mut command, "extracted Cargo archive consumer compilation")?;
        if rust_library {
            let consumer = extracted.directory.path().join("consumer");
            fs::create_dir_all(consumer.join("src"))?;
            let mut manifest = DocumentMut::new();
            manifest["package"]["name"] = value("rcr-archive-consumer");
            manifest["package"]["version"] = value("0.0.0");
            manifest["package"]["edition"] = value("2024");
            manifest["workspace"] = Item::Table(toml_edit::Table::new());
            let dependency = &mut manifest["dependencies"]["rcr_archive"];
            dependency["package"] = value(artifact.name());
            dependency["version"] = value(format!("={}", artifact.version()));
            dependency["path"] = value(extracted.root.to_string_lossy().as_ref());
            fs::write(consumer.join("Cargo.toml"), manifest.to_string())?;
            fs::write(
                consumer.join("src/main.rs"),
                "use rcr_archive as _;\nfn main() {}\n",
            )?;
            run_cargo(
                Command::new("cargo")
                    .current_dir(&consumer)
                    .args(["generate-lockfile", "--offline"]),
                "isolated archive consumer lockfile",
            )?;
            run_cargo(
                Command::new("cargo")
                    .current_dir(&consumer)
                    .args(["build", "--locked", "--offline", "--target-dir"])
                    .arg(extracted.directory.path().join("compiled")),
                "isolated archive dependency consumer",
            )?;
        }
        artifact.verify()
    }
}

pub(crate) fn checksum(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn validate_name(name: &str) -> Result {
    if name.is_empty()
        || name.len() > 64
        || !name.as_bytes()[0].is_ascii_alphabetic()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(Error::invalid("invalid Cargo package name"));
    }
    Ok(())
}

fn parse_document(source: &str) -> Result<DocumentMut> {
    if source.contains("\r\n") && source.replace("\r\n", "").contains('\n') {
        return Err(Error::invalid(
            "mixed TOML newline styles are not supported",
        ));
    }
    source
        .replace("\r\n", "\n")
        .parse()
        .map_err(|error| Error::invalid(format!("invalid Cargo TOML: {error}")))
}

fn render_document(document: &DocumentMut, original: &str) -> String {
    let output = document.to_string();
    if original.contains("\r\n") {
        output.replace('\n', "\r\n")
    } else {
        output
    }
}

fn replace_string(item: &mut Item, replacement: &str) -> Result {
    let decor = item
        .as_value()
        .ok_or_else(|| Error::invalid("version must be a TOML string"))?
        .decor()
        .clone();
    *item = value(replacement);
    *item.as_value_mut().unwrap().decor_mut() = decor;
    Ok(())
}

fn run_cargo(command: &mut Command, operation: &str) -> Result<Vec<u8>> {
    let output = command
        .output()
        .map_err(|error| Error::from(error).context(operation))?;
    if !output.status.success() {
        return Err(Error::invalid(format!(
            "{operation} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

fn cargo_metadata(manifest: &Path) -> Result<Value> {
    let mut command = Command::new("cargo");
    command
        .current_dir(
            manifest
                .parent()
                .ok_or_else(|| Error::invalid("manifest has no parent"))?,
        )
        .args([
            "metadata",
            "--locked",
            "--offline",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(manifest);
    let output = run_cargo(&mut command, "Cargo package metadata")?;
    Ok(serde_json::from_slice(&output)?)
}

fn selected_package<'a>(all: &'a Value, manifest: &Path) -> Result<&'a Value> {
    let canonical = dunce::canonicalize(manifest)?;
    let packages = all["packages"]
        .as_array()
        .ok_or_else(|| Error::invalid("Cargo metadata has no packages"))?;
    let matches: Vec<_> = packages
        .iter()
        .filter(|package| {
            package["manifest_path"]
                .as_str()
                .and_then(|path| dunce::canonicalize(path).ok())
                .as_ref()
                == Some(&canonical)
        })
        .collect();
    if matches.len() != 1 {
        return Err(Error::invalid(
            "Cargo metadata does not identify exactly one selected package",
        ));
    }
    Ok(matches[0])
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::invalid(format!("Cargo metadata is missing string field {key}")))
}

fn validate_publish(package: &Value) -> Result {
    match package.get("publish") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Array(registries)) if registries.iter().any(|name| name == "crates-io") => {
            Ok(())
        }
        Some(Value::Array(_)) => Err(Error::invalid(
            "package publication to crates.io is disabled",
        )),
        _ => Err(Error::invalid("invalid Cargo publish restriction")),
    }
}

struct Extracted {
    directory: tempfile::TempDir,
    root: PathBuf,
}

fn archive_directory() -> Result<tempfile::TempDir> {
    let directory = tempfile::Builder::new()
        .prefix(".rcr-archive-")
        .tempdir()
        .map_err(|error| {
            Error::from(error).context("creating isolated Cargo archive scratch directory")
        })?;
    let canonical = dunce::canonicalize(directory.path())?;
    for ancestor in canonical.ancestors().skip(1) {
        if ancestor.join("Cargo.toml").try_exists()? {
            return Err(Error::invalid(
                "the OS temporary directory is inside a Cargo project; configure an external temporary directory",
            ));
        }
    }
    Ok(directory)
}

fn extract(bytes: &[u8]) -> Result<Extracted> {
    let directory = archive_directory()?;
    let mut decoder = GzDecoder::new(bytes);
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded)?;
    if !decoder.into_inner().is_empty() {
        return Err(Error::invalid("Cargo archive contains trailing gzip data"));
    }
    let mut archive = Archive::new(decoded.as_slice());
    let mut seen = BTreeSet::new();
    let mut top = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let components: Vec<_> = path.components().collect();
        if components.len() < 2
            || components
                .iter()
                .any(|part| !matches!(part, Component::Normal(_)))
            || path.to_str().is_none()
            || path.to_string_lossy().contains(['\\', ':'])
            || !(entry.header().entry_type().is_file() || entry.header().entry_type().is_dir())
            || !seen.insert(path.clone())
        {
            return Err(Error::invalid("unsafe or duplicate Cargo archive entry"));
        }
        let first = components[0].as_os_str().to_os_string();
        if top.as_ref().is_some_and(|expected| expected != &first) {
            return Err(Error::invalid(
                "Cargo archive contains multiple package roots",
            ));
        }
        top = Some(first);
        if !entry.unpack_in(directory.path())? {
            return Err(Error::invalid(
                "Cargo archive entry escaped extraction directory",
            ));
        }
    }
    let root = dunce::canonicalize(
        directory
            .path()
            .join(top.ok_or_else(|| Error::invalid("empty Cargo archive"))?),
    )?;
    if !root.join("Cargo.toml").is_file() {
        return Err(Error::invalid("Cargo archive has no normalized Cargo.toml"));
    }
    Ok(Extracted { directory, root })
}

struct Inspected {
    name: String,
    version: Version,
    upload_metadata: Value,
}

fn inspect(bytes: &[u8]) -> Result<Inspected> {
    let extracted = extract(bytes)?;
    let manifest = extracted.root.join("Cargo.toml");
    let document = parse_document(&fs::read_to_string(&manifest)?)?;
    validate_normalized_manifest(&document)?;
    let all = cargo_metadata(&manifest)?;
    let package = selected_package(&all, &manifest)?;
    validate_publish(package)?;
    let name = required_string(package, "name")?.to_owned();
    validate_name(&name)?;
    let version = Version::parse(required_string(package, "version")?)?;
    for target in package["targets"]
        .as_array()
        .ok_or_else(|| Error::invalid("archive contains no targets"))?
    {
        let source = dunce::canonicalize(required_string(target, "src_path")?)?;
        if !source.starts_with(&extracted.root) {
            return Err(Error::invalid("Cargo target source escapes the archive"));
        }
    }
    if extracted.root.file_name().and_then(|name| name.to_str())
        != Some(format!("{name}-{version}").as_str())
    {
        return Err(Error::invalid(
            "archive root disagrees with normalized Cargo package identity",
        ));
    }
    let dependencies = package["dependencies"]
        .as_array()
        .ok_or_else(|| Error::invalid("Cargo metadata has no dependency list"))?;
    let mut deps = Vec::new();
    for dependency in dependencies {
        deps.push(upload_dependency(dependency)?);
    }
    let (readme_file, readme) = file_metadata(package, "readme", &extracted.root)?;
    let (license_file, _) = file_metadata(package, "license_file", &extracted.root)?;
    let mut badges = serde_json::Map::new();
    if let Some(table) = document.get("badges").and_then(Item::as_table_like) {
        for (name, badge) in table.iter() {
            let mut fields = serde_json::Map::new();
            let badge = badge
                .as_table_like()
                .ok_or_else(|| Error::invalid("invalid Cargo badge"))?;
            for (key, value) in badge.iter() {
                fields.insert(
                    key.into(),
                    Value::String(
                        value
                            .as_str()
                            .ok_or_else(|| Error::invalid("invalid Cargo badge field"))?
                            .into(),
                    ),
                );
            }
            badges.insert(name.into(), Value::Object(fields));
        }
    }
    let upload_metadata = json!({
        "name": name,
        "vers": version.to_string(),
        "deps": deps,
        "features": package["features"],
        "authors": package["authors"],
        "description": package["description"],
        "documentation": package["documentation"],
        "homepage": package["homepage"],
        "readme": readme,
        "readme_file": readme_file,
        "keywords": package["keywords"],
        "categories": package["categories"],
        "license": package["license"],
        "license_file": license_file,
        "repository": package["repository"],
        "badges": badges,
        "links": package["links"],
        "rust_version": package["rust_version"],
    });
    Ok(Inspected {
        name,
        version,
        upload_metadata,
    })
}

fn upload_dependency(dependency: &Value) -> Result<Value> {
    let source = dependency["source"].as_str();
    let registry = dependency["registry"].as_str().or_else(|| {
        source.and_then(|source| {
            source
                .strip_prefix("registry+")
                .or_else(|| source.starts_with("sparse+").then_some(source))
        })
    });
    if source.is_some_and(|source| source.starts_with("git+")) || dependency["path"].is_string() {
        return Err(Error::invalid(
            "normalized archive contains a non-registry dependency",
        ));
    }
    let registry = registry.filter(|url| {
        !matches!(
            *url,
            "https://github.com/rust-lang/crates.io-index"
                | "https://index.crates.io/"
                | "sparse+https://index.crates.io/"
        )
    });
    Ok(json!({
        "name": required_string(dependency, "name")?,
        "version_req": required_string(dependency, "req")?,
        "features": dependency["features"],
        "optional": dependency["optional"],
        "default_features": dependency["uses_default_features"],
        "target": dependency["target"],
        "kind": dependency["kind"].as_str().unwrap_or("normal"),
        "registry": registry,
        "explicit_name_in_toml": dependency["rename"],
    }))
}

fn validate_normalized_manifest(document: &DocumentMut) -> Result {
    let package = document
        .get("package")
        .and_then(Item::as_table_like)
        .ok_or_else(|| Error::invalid("archive has no package manifest"))?;
    if package.get("name").and_then(Item::as_str).is_none()
        || package.get("version").and_then(Item::as_str).is_none()
        || package.get("workspace").is_some()
        || document.get("patch").is_some()
        || document.get("replace").is_some()
    {
        return Err(Error::invalid(
            "archive manifest must describe one normalized package",
        ));
    }
    if let Some(workspace) = document.get("workspace").and_then(Item::as_table_like)
        && workspace.iter().any(|(key, _)| key != "resolver")
    {
        return Err(Error::invalid(
            "archive manifest cannot select workspace members or inherit workspace values",
        ));
    }
    fn dependencies(table: &dyn toml_edit::TableLike) -> Result {
        for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
            if let Some(dependencies) = table.get(key).and_then(Item::as_table_like) {
                for (_, dependency) in dependencies.iter() {
                    if let Some(fields) = dependency.as_table_like()
                        && ["path", "git", "workspace"]
                            .iter()
                            .any(|key| fields.get(key).is_some())
                    {
                        return Err(Error::invalid(
                            "archive dependencies must be normalized registry dependencies",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
    dependencies(document.as_table())?;
    if let Some(targets) = document.get("target").and_then(Item::as_table_like) {
        for (_, target) in targets.iter() {
            dependencies(
                target
                    .as_table_like()
                    .ok_or_else(|| Error::invalid("invalid Cargo target dependencies"))?,
            )?;
        }
    }
    Ok(())
}

fn file_metadata(
    package: &Value,
    field: &str,
    root: &Path,
) -> Result<(Option<String>, Option<String>)> {
    let Some(relative) = package[field].as_str() else {
        return Ok((None, None));
    };
    let path = Path::new(relative);
    if path
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
        || relative.contains('\\')
        || relative.is_empty()
    {
        return Err(Error::invalid(format!(
            "archive {field} must be a contained relative path"
        )));
    }
    let contents = fs::read_to_string(root.join(path))?;
    Ok((Some(relative.into()), Some(contents)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::sync::OnceLock;
    use tar::{Builder, Header};

    pub(crate) fn fixture(name: &str, binary: bool) -> tempfile::TempDir {
        let parent = std::env::current_dir()
            .unwrap()
            .join("target/rcr-package-tests");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("project-")
            .tempdir_in(parent)
            .unwrap();
        fs::create_dir(directory.path().join("src")).unwrap();
        fs::write(
            directory.path().join("Cargo.toml"),
            format!(
                "# Preserve this comment.\n[workspace]\n\n[package]\nname = \"{name}\"\nversion = \"1.2.3\" # Preserve version comment.\nedition = \"2024\"\nrust-version = \"1.99.1\"\ndescription = \"An isolated Cargo archive fixture\"\nlicense-file = \"LICENSE\"\nreadme = \"README.md\"\ninclude = [\"src/**\", \"Cargo.toml\", \"Cargo.lock\", \"README.md\", \"LICENSE\"]\n"
            ),
        ).unwrap();
        fs::write(
            directory.path().join("README.md"),
            "# Archive Fixture\n\nRetained README.\n",
        )
        .unwrap();
        fs::write(directory.path().join("LICENSE"), "A fixture license.\n").unwrap();
        fs::write(
            directory
                .path()
                .join(if binary { "src/main.rs" } else { "src/lib.rs" }),
            if binary {
                "fn main() { println!(\"archive\"); }\n"
            } else {
                "pub fn answer() -> u32 { 42 }\n"
            },
        )
        .unwrap();
        run_cargo(
            Command::new("cargo")
                .current_dir(directory.path())
                .args(["generate-lockfile", "--offline"]),
            "fixture lockfile",
        )
        .unwrap();
        directory
    }

    fn archive_fixture(bytes: &[u8]) -> (tempfile::TempDir, Artifact) {
        let parent = std::env::current_dir()
            .unwrap()
            .join("target/rcr-package-tests");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("retained-")
            .tempdir_in(parent)
            .unwrap();
        let path = directory.path().join("fixture.crate");
        fs::write(&path, bytes).unwrap();
        let artifact = Artifact::open(&path).unwrap();
        (directory, artifact)
    }

    pub(crate) fn retained_fixture() -> (tempfile::TempDir, Artifact) {
        static FIXTURE: OnceLock<Vec<u8>> = OnceLock::new();
        let bytes = FIXTURE.get_or_init(|| {
            let directory = fixture("rcr-retained-fixture", false);
            let package = CargoPackage::new(directory.path()).unwrap();
            let artifact = package
                .build(&directory.path().join("artifacts"), 1_700_000_000)
                .unwrap();
            fs::read(artifact.path()).unwrap()
        });
        archive_fixture(bytes)
    }

    #[test]
    fn real_library_and_binary_archives_compile_without_workspace_sources() {
        for binary in [false, true] {
            let fixture = fixture(
                if binary {
                    "rcr-bin-fixture"
                } else {
                    "rcr-lib-fixture"
                },
                binary,
            );
            let package = CargoPackage::new(fixture.path()).unwrap();
            let artifact = package
                .build(&fixture.path().join("artifacts"), 1_700_000_000)
                .unwrap();
            assert_eq!(artifact.name(), package.metadata().unwrap().name);
            assert_eq!(
                artifact.upload_metadata()["readme"],
                "# Archive Fixture\n\nRetained README.\n"
            );
            assert_eq!(artifact.upload_metadata()["readme_file"], "README.md");
            assert_eq!(artifact.upload_metadata()["license_file"], "LICENSE");
            assert_eq!(artifact.upload_metadata()["rust_version"], "1.99.1");
            fs::write(
                fixture
                    .path()
                    .join(if binary { "src/main.rs" } else { "src/lib.rs" }),
                "not valid Rust",
            )
            .unwrap();
            package.verify_consumer(&artifact).unwrap();
            package.set_version(&Version::new(1, 2, 4)).unwrap();
            assert_eq!(artifact.version(), &Version::new(1, 2, 3));
            artifact.verify().unwrap();
        }
    }

    #[test]
    fn version_and_lock_updates_preserve_other_source_and_newlines() {
        for newline in ["\n", "\r\n"] {
            let fixture = fixture("rcr-version-fixture", false);
            let manifest = fixture.path().join("Cargo.toml");
            let lock = fixture.path().join("Cargo.lock");
            let source = format!(
                "{}\n[dependencies]\nsemver = \"1.0.28\"\n",
                fs::read_to_string(&manifest).unwrap()
            );
            fs::write(&manifest, source).unwrap();
            run_cargo(
                Command::new("cargo")
                    .current_dir(fixture.path())
                    .args(["generate-lockfile", "--offline"]),
                "version fixture dependency lockfile",
            )
            .unwrap();
            for path in [&manifest, &lock] {
                let source = fs::read_to_string(path).unwrap().replace('\n', newline);
                fs::write(path, source).unwrap();
            }
            let original_manifest = fs::read_to_string(&manifest).unwrap();
            let original_lock = fs::read_to_string(&lock).unwrap();
            let package = CargoPackage::new(fixture.path()).unwrap();
            package.set_version(&Version::new(2, 0, 0)).unwrap();
            assert_eq!(
                fs::read_to_string(&manifest).unwrap(),
                original_manifest.replace("version = \"1.2.3\"", "version = \"2.0.0\""),
            );
            assert_eq!(
                fs::read_to_string(&lock).unwrap(),
                original_lock.replace("version = \"1.2.3\"", "version = \"2.0.0\""),
            );
            assert_eq!(package.metadata().unwrap().version, Version::new(2, 0, 0));
            let unchanged = fs::read(&lock).unwrap();
            package.set_version(&Version::new(2, 0, 0)).unwrap();
            assert_eq!(unchanged, fs::read(&lock).unwrap());
        }
    }

    #[test]
    fn explicit_package_selection_and_inherited_version_fail_closed() {
        let fixture = fixture("rcr-workspace-fixture", false);
        fs::write(
            fixture.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\n[workspace.package]\nversion = \"1.2.3\"\n",
        )
        .unwrap();
        assert!(CargoPackage::new(fixture.path()).is_err());
        fs::create_dir_all(fixture.path().join("member/src")).unwrap();
        fs::write(
            fixture.path().join("member/Cargo.toml"),
            "[package]\nname = \"rcr-member-fixture\"\nversion.workspace = true\nedition = \"2024\"\n",
        ).unwrap();
        fs::write(fixture.path().join("member/src/lib.rs"), "").unwrap();
        let package =
            CargoPackage::from_manifest(fixture.path().join("member/Cargo.toml")).unwrap();
        let before = fs::read(fixture.path().join("member/Cargo.toml")).unwrap();
        assert!(
            package
                .set_version(&Version::new(2, 0, 0))
                .unwrap_err()
                .to_string()
                .contains("inherited")
        );
        assert_eq!(
            before,
            fs::read(fixture.path().join("member/Cargo.toml")).unwrap()
        );
    }

    #[test]
    fn disabled_publication_missing_locks_and_bad_versions_do_not_edit_sources() {
        for restriction in ["false", "[\"private-registry\"]"] {
            let fixture = fixture("rcr-private-fixture", false);
            let manifest = fixture.path().join("Cargo.toml");
            let source = format!(
                "{}publish = {restriction}\n",
                fs::read_to_string(&manifest).unwrap()
            );
            fs::write(&manifest, &source).unwrap();
            let package = CargoPackage::new(fixture.path()).unwrap();
            assert!(package.metadata().is_err());
            assert!(package.build(&fixture.path().join("artifacts"), 0).is_err());
            assert_eq!(source, fs::read_to_string(&manifest).unwrap());
        }
        let fixture = fixture("rcr-missing-lock-fixture", false);
        let package = CargoPackage::new(fixture.path()).unwrap();
        let manifest = fs::read(fixture.path().join("Cargo.toml")).unwrap();
        fs::remove_file(fixture.path().join("Cargo.lock")).unwrap();
        assert!(package.set_version(&Version::new(2, 0, 0)).is_err());
        assert!(package.build(&fixture.path().join("artifacts"), 0).is_err());
        assert_eq!(
            manifest,
            fs::read(fixture.path().join("Cargo.toml")).unwrap()
        );
        assert!(!fixture.path().join("Cargo.lock").exists());
        assert!(
            package
                .build(&fixture.path().join("artifacts"), -1)
                .is_err()
        );
    }

    #[test]
    fn retained_artifacts_reconcile_only_identical_bytes_and_detect_corruption() {
        let fixture = fixture("rcr-conflict-fixture", false);
        let package = CargoPackage::new(fixture.path()).unwrap();
        let directory = fixture.path().join("artifacts");
        let artifact = package.build(&directory, 1_700_000_000).unwrap();
        let original = fs::read(artifact.path()).unwrap();
        let again = package.build(&directory, 1_700_000_000).unwrap();
        assert_eq!(artifact.checksum(), again.checksum());
        fs::write(artifact.path(), b"conflicting retained bytes").unwrap();
        assert!(artifact.verify().is_err());
        assert!(package.build(&directory, 1_700_000_000).is_err());
        assert_eq!(
            fs::read(artifact.path()).unwrap(),
            b"conflicting retained bytes"
        );
        fs::write(artifact.path(), &original).unwrap();
        artifact.verify().unwrap();
        let mut bad = original;
        let end = bad.len() - 1;
        bad[end] ^= 0xff;
        fs::write(artifact.path(), &bad).unwrap();
        assert!(Artifact::open(artifact.path()).is_err());
    }

    fn archive(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut archive = Builder::new(encoder);
        for (path, kind, contents) in entries {
            let mut header = Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            if kind.is_symlink() || kind.is_hard_link() {
                header.set_link_name("Cargo.toml").unwrap();
            }
            header.set_cksum();
            archive.append_data(&mut header, path, *contents).unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap()
    }

    fn vcs_archive(info: Option<&[u8]>) -> (tempfile::TempDir, Artifact) {
        let mut entries: Vec<(&str, tar::EntryType, &[u8])> =
            vec![
            (
                "rcr-vcs-fixture-1.0.0/Cargo.toml",
                tar::EntryType::Regular,
                b"[package]\nname = \"rcr-vcs-fixture\"\nversion = \"1.0.0\"\nedition = \"2024\"\n",
            ),
            (
                "rcr-vcs-fixture-1.0.0/Cargo.lock",
                tar::EntryType::Regular,
                b"version = 4\n[[package]]\nname = \"rcr-vcs-fixture\"\nversion = \"1.0.0\"\n",
            ),
            ("rcr-vcs-fixture-1.0.0/src/lib.rs", tar::EntryType::Regular, b""),
        ];
        if let Some(info) = info {
            entries.push((
                "rcr-vcs-fixture-1.0.0/.cargo_vcs_info.json",
                tar::EntryType::Regular,
                info,
            ));
        }
        archive_fixture(&archive(&entries))
    }

    #[test]
    fn vcs_commit_reads_complete_clean_sha1_and_sha256_metadata() {
        for info in [
            json!({"git": {"sha1": "ABCDEF1234".repeat(4)}, "path_in_vcs": ""}),
            json!({"git": {"sha1": "1234abcd".repeat(8), "dirty": false}, "path_in_vcs": "member"}),
        ] {
            let bytes = serde_json::to_vec(&info).unwrap();
            let (_retained, artifact) = vcs_archive(Some(&bytes));
            assert_eq!(
                artifact.vcs_commit().unwrap(),
                Some(info["git"]["sha1"].as_str().unwrap().to_ascii_lowercase()),
            );
        }
    }

    #[test]
    fn vcs_commit_rejects_dirty_and_malformed_info_without_rejecting_artifact_open() {
        let mut infos: Vec<_> = [
            json!({"git": {"sha1": "a".repeat(40), "dirty": true}}),
            json!({"git": {"sha1": "a".repeat(40), "dirty": "false"}}),
            json!({"git": {"sha1": "a".repeat(40), "dirty": null}}),
            json!({"git": {"sha1": "a".repeat(39)}}),
            json!({"git": {"sha1": "a".repeat(65)}}),
            json!({"git": {"sha1": "g".repeat(40)}}),
            json!({"git": {"sha1": 123}}),
            json!({"git": {}}),
            json!({"git": null}),
            json!({}),
            json!([]),
        ]
        .iter()
        .map(|info| serde_json::to_vec(info).unwrap())
        .collect();
        infos.push(b"not valid JSON".to_vec());
        for info in infos {
            let (_retained, artifact) = vcs_archive(Some(&info));
            artifact.verify().unwrap();
            assert!(artifact.vcs_commit().is_err());
        }
    }

    #[test]
    fn vcs_commit_absence_is_optional_but_retained_byte_changes_fail() {
        let (_retained, artifact) = vcs_archive(None);
        assert_eq!(artifact.vcs_commit().unwrap(), None);
        fs::write(artifact.path(), b"changed retained bytes").unwrap();
        assert!(
            artifact
                .vcs_commit()
                .unwrap_err()
                .to_string()
                .contains("checksum")
        );
    }

    #[test]
    fn archive_consumers_reject_broken_archive_code_even_with_valid_workspace_sources() {
        for binary in [false, true] {
            let fixture = fixture("rcr-broken-archive", binary);
            let package = CargoPackage::new(fixture.path()).unwrap();
            let manifest = fs::read(fixture.path().join("Cargo.toml")).unwrap();
            let lock = fs::read(fixture.path().join("Cargo.lock")).unwrap();
            let source = if binary {
                "rcr-broken-archive-1.2.3/src/main.rs"
            } else {
                "rcr-broken-archive-1.2.3/src/lib.rs"
            };
            let bytes = archive(&[
                (
                    "rcr-broken-archive-1.2.3/Cargo.toml",
                    tar::EntryType::Regular,
                    &manifest,
                ),
                (
                    "rcr-broken-archive-1.2.3/Cargo.lock",
                    tar::EntryType::Regular,
                    &lock,
                ),
                (
                    "rcr-broken-archive-1.2.3/README.md",
                    tar::EntryType::Regular,
                    b"Archive README.",
                ),
                (
                    "rcr-broken-archive-1.2.3/LICENSE",
                    tar::EntryType::Regular,
                    b"Archive license.",
                ),
                (source, tar::EntryType::Regular, b"not valid Rust"),
            ]);
            let (_retained, artifact) = archive_fixture(&bytes);
            assert!(
                package
                    .verify_consumer(&artifact)
                    .unwrap_err()
                    .to_string()
                    .contains("compilation")
            );
            let original = fs::read_to_string(fixture.path().join(if binary {
                "src/main.rs"
            } else {
                "src/lib.rs"
            }))
            .unwrap();
            assert!(!original.contains("not valid Rust"));
        }
    }

    #[test]
    fn traversal_entries_and_external_manifest_targets_are_rejected() {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = Builder::new(encoder);
        let mut header = Header::new_gnu();
        let path = b"foo-1.0.0/../../escape";
        header.as_mut_bytes()[..path.len()].copy_from_slice(path);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(0);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, b"".as_slice()).unwrap();
        let bytes = builder.into_inner().unwrap().finish().unwrap();
        assert!(extract(&bytes).is_err());
        let bytes = archive(&[
            ("foo-1.0.0/Cargo.toml", tar::EntryType::Regular,
                b"[package]\nname = \"foo\"\nversion = \"1.0.0\"\n[lib]\npath = \"../../outside.rs\"\n"),
            ("foo-1.0.0/Cargo.lock", tar::EntryType::Regular,
                b"version = 4\n[[package]]\nname = \"foo\"\nversion = \"1.0.0\"\n"),
        ]);
        assert!(inspect(&bytes).is_err());
    }

    #[test]
    fn archive_publish_restrictions_and_source_overrides_fail_before_consumption() {
        for manifest in [
            "[package]\nname = \"foo\"\nversion = \"1.0.0\"\npublish = false\n",
            "[package]\nname = \"foo\"\nversion = \"1.0.0\"\npublish = [\"private-registry\"]\n",
            "[package]\nname = \"foo\"\nversion = \"1.0.0\"\n[patch.crates-io]\nfoo = { path = \"../outside\" }\n",
        ] {
            let bytes = archive(&[
                (
                    "foo-1.0.0/Cargo.toml",
                    tar::EntryType::Regular,
                    manifest.as_bytes(),
                ),
                (
                    "foo-1.0.0/Cargo.lock",
                    tar::EntryType::Regular,
                    b"version = 4\n[[package]]\nname = \"foo\"\nversion = \"1.0.0\"\n",
                ),
                ("foo-1.0.0/src/lib.rs", tar::EntryType::Regular, b""),
            ]);
            assert!(inspect(&bytes).is_err());
        }
    }

    #[test]
    fn archive_extraction_rejects_duplicate_links_roots_and_non_normalized_manifests() {
        for bytes in [
            archive(&[
                ("foo-1.0.0/Cargo.toml", tar::EntryType::Regular, b""),
                ("foo-1.0.0/Cargo.toml", tar::EntryType::Regular, b""),
            ]),
            archive(&[("foo-1.0.0/link", tar::EntryType::Symlink, b"")]),
            archive(&[("foo-1.0.0/link", tar::EntryType::Link, b"")]),
            archive(&[
                ("foo-1.0.0/Cargo.toml", tar::EntryType::Regular, b""),
                ("other-1.0.0/file", tar::EntryType::Regular, b""),
            ]),
            archive(&[("foo-1.0.0/Cargo.toml", tar::EntryType::Regular,
                b"[package]\nname = \"foo\"\nversion = \"1.0.0\"\n[dependencies]\nescape = { path = \"../outside\" }\n")]),
            archive(&[("foo-1.0.0/Cargo.toml", tar::EntryType::Regular,
                b"[package]\nname = \"foo\"\nversion = \"1.0.0\"\n[workspace]\nmembers = [\"../../outside\"]\n")]),
            b"not a gzip archive".to_vec(),
        ] {
            assert!(inspect(&bytes).is_err());
        }
        let (_fixture, artifact) = retained_fixture();
        let mut bytes = fs::read(artifact.path()).unwrap();
        bytes.extend_from_slice(b"trailing");
        assert!(inspect(&bytes).is_err());
    }

    pub(crate) fn metadata_fixture() -> (tempfile::TempDir, Artifact) {
        static FIXTURE: OnceLock<Vec<u8>> = OnceLock::new();
        let bytes = FIXTURE.get_or_init(|| {
            let fixture = fixture("rcr-metadata-fixture", false);
            let manifest = fixture.path().join("Cargo.toml");
            let mut source = fs::read_to_string(&manifest)
                .unwrap()
                .replace("include = [", "include = [\"build.rs\", ");
            source.push_str(
                "\n[dependencies]\nrenamed = { package = \"serde\", version = \"1.0.229\", optional = true, default-features = false, features = [\"derive\"] }\n\
                 [dev-dependencies]\nserde_json = \"1.0.154\"\n\
                 [build-dependencies]\nsemver = \"1.0.28\"\n\
                 [target.'cfg(windows)'.dependencies]\nthiserror = \"2.0.21\"\n\
                 [features]\nextra = [\"dep:renamed\"]\n",
            );
            fs::write(&manifest, source).unwrap();
            fs::write(
                fixture.path().join("build.rs"),
                "fn main() {\n\
                 let version = semver::Version::parse(env!(\"CARGO_PKG_VERSION\")).unwrap();\n\
                 println!(\"cargo::rustc-env=FIXTURE_VERSION={version}\");\n\
                 }\n",
            )
            .unwrap();
            fs::write(
                fixture.path().join("src/lib.rs"),
                "pub fn answer() -> u32 { 42 }\n\
                 pub const VERSION: &str = env!(\"FIXTURE_VERSION\");\n\
                 #[cfg(windows)]\n\
                 #[derive(Debug, thiserror::Error)]\n\
                 #[error(\"metadata fixture error\")]\n\
                 pub struct FixtureError;\n",
            )
            .unwrap();
            run_cargo(
                Command::new("cargo").current_dir(fixture.path()).args(["generate-lockfile", "--offline"]),
                "dependency fixture lockfile",
            ).unwrap();
            let artifact = CargoPackage::new(fixture.path()).unwrap()
                .build(&fixture.path().join("artifacts"), 1_700_000_000).unwrap();
            fs::read(artifact.path()).unwrap()
        });
        archive_fixture(bytes)
    }

    #[test]
    fn normalized_archive_metadata_preserves_renamed_target_and_dependency_kinds() {
        let (_fixture, artifact) = metadata_fixture();
        let metadata = artifact.upload_metadata();
        let deps = metadata["deps"].as_array().unwrap();
        let renamed = deps.iter().find(|dep| dep["name"] == "serde").unwrap();
        assert_eq!(renamed["explicit_name_in_toml"], "renamed");
        assert_eq!(renamed["optional"], true);
        assert_eq!(renamed["default_features"], false);
        assert_eq!(renamed["features"], json!(["derive"]));
        assert_eq!(renamed["kind"], "normal");
        assert!(renamed["registry"].is_null());
        assert!(
            deps.iter()
                .any(|dep| dep["name"] == "serde_json" && dep["kind"] == "dev")
        );
        assert!(
            deps.iter()
                .any(|dep| dep["name"] == "semver" && dep["kind"] == "build")
        );
        assert!(
            deps.iter()
                .any(|dep| dep["name"] == "thiserror" && dep["target"] == "cfg(windows)")
        );
        assert_eq!(metadata["features"]["extra"], json!(["dep:renamed"]));
    }

    #[test]
    fn dependency_registry_mapping_preserves_sparse_indexes_and_rejects_source_paths() {
        let mut dependency = json!({
            "name": "example",
            "req": "^1.0",
            "source": "registry+sparse+https://registry.example/index/",
            "registry": null,
            "features": [],
            "optional": false,
            "uses_default_features": true,
            "target": null,
            "kind": null,
            "rename": null,
        });
        assert_eq!(
            upload_dependency(&dependency).unwrap()["registry"],
            "sparse+https://registry.example/index/"
        );
        dependency["registry"] = json!("https://explicit.example/index");
        assert_eq!(
            upload_dependency(&dependency).unwrap()["registry"],
            "https://explicit.example/index"
        );
        dependency["registry"] = Value::Null;
        dependency["source"] = json!("sparse+https://index.crates.io/");
        assert!(upload_dependency(&dependency).unwrap()["registry"].is_null());
        dependency["source"] = json!("git+https://example.invalid/repository");
        assert!(upload_dependency(&dependency).is_err());
        dependency["source"] = Value::Null;
        dependency["path"] = json!("../outside");
        assert!(upload_dependency(&dependency).is_err());
    }
}
