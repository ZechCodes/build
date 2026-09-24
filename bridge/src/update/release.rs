//! Download a signed release and stage its one executable for the helper.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::Release;
use super::UpdateBackend;
use async_trait::async_trait;

const DEFAULT_REPO: &str = "ZechCodes/build-releases";
const COSIGN_VERSION: &str = "v3.1.3";
const COSIGN_ISSUER: &str = "https://token.actions.githubusercontent.com";
/// The repositories whose release workflow may sign a bridge release: the
/// current name and the name it is being renamed to. Each is matched as an
/// exact certificate identity, never a pattern.
const RELEASE_REPOSITORIES: [&str; 2] = ["ZechCodes/build-web", "ZechCodes/build"];
const MAX_ARCHIVE: usize = 128 * 1024 * 1024;
const MAX_MANIFEST: usize = 1024 * 1024;
const MAX_COSIGN: usize = 160 * 1024 * 1024;
const MAX_BINARY: u64 = 256 * 1024 * 1024;

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    published_at: Option<String>,
}

pub fn platform_key() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("linux", "aarch64") => Ok("linux-aarch64"),
        ("macos", "x86_64") => Ok("macos-x86_64"),
        ("macos", "aarch64") => Ok("macos-arm64"),
        (os, arch) => Err(format!("no bridge release for {os}/{arch}")),
    }
}

fn release_repo() -> Result<String, String> {
    let repo = std::env::var("BUILD_RELEASES_REPO")
        .or_else(|_| std::env::var("RELEASES_REPO"))
        .unwrap_or_else(|_| DEFAULT_REPO.into());
    let mut parts = repo.split('/');
    let valid = parts.clone().count() == 2
        && parts.all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        });
    if valid {
        Ok(repo)
    } else {
        Err("invalid BUILD_RELEASES_REPO".into())
    }
}

pub async fn latest(client: &Client) -> Result<Release, String> {
    let repo = release_repo()?;
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    latest_from_url(client, &url).await
}

async fn latest_from_url(client: &Client, url: &str) -> Result<Release, String> {
    let response = client
        .get(url)
        .header("User-Agent", "build-bridge-updater")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let payload: GitHubRelease = response.json().await.map_err(|e| e.to_string())?;
    let version = payload
        .tag_name
        .strip_prefix("bridge-v")
        .ok_or("latest release is not a bridge tag")?;
    semver::Version::parse(version).map_err(|e| format!("invalid bridge tag: {e}"))?;
    Ok(Release {
        version: version.to_string(),
        tag: payload.tag_name,
        published_at: payload.published_at,
    })
}

async fn download(client: &Client, url: &str, limit: usize) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .header("User-Agent", "build-bridge-updater")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    if response
        .content_length()
        .is_some_and(|len| len > limit as u64)
    {
        return Err("release asset is too large".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if chunk.len() > limit - bytes.len() {
            return Err("release asset is too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn cosign_asset() -> Result<(&'static str, &'static str), String> {
    match platform_key()? {
        "linux-x86_64" => Ok((
            "cosign-linux-amd64",
            "4629c757b7618056f8ddd7e2625ae9fdd94c0372a65049520bc7d9df9efc7f71",
        )),
        "linux-aarch64" => Ok((
            "cosign-linux-arm64",
            "c5d324e091826b0d7a78eb16fef316450b4eb9aaec045611c08ba06f5e73220a",
        )),
        "macos-x86_64" => Ok((
            "cosign-darwin-amd64",
            "2347488e5d5b25336644024dfeca5601b190e91197a71a917bda44744aff106c",
        )),
        "macos-arm64" => Ok((
            "cosign-darwin-arm64",
            "5cf948c2f4dfe59687bdd0b8523709067383e03982cc543475c8a7dc70e92a76",
        )),
        _ => Err("unsupported cosign platform".into()),
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

async fn verified_cosign(client: &Client, cache: &Path) -> Result<PathBuf, String> {
    let (asset, expected) = cosign_asset()?;
    fs::create_dir_all(cache).map_err(|e| e.to_string())?;
    let path = cache.join(format!("{COSIGN_VERSION}-{asset}"));
    if fs::read(&path).is_ok_and(|bytes| digest(&bytes) == expected) {
        return Ok(path);
    }
    let url =
        format!("https://github.com/sigstore/cosign/releases/download/{COSIGN_VERSION}/{asset}");
    let bytes = download(client, &url, MAX_COSIGN).await?;
    if digest(&bytes) != expected {
        return Err("pinned cosign digest mismatch".into());
    }
    let temporary = path.with_extension(format!("part-{}", uuid::Uuid::new_v4()));
    write_private(&temporary, &bytes, 0o700)?;
    fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

fn write_private(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

fn verify_signature(cosign: &Path, sums: &Path, bundle: &Path, tag: &str) -> Result<(), String> {
    if !tag.starts_with("bridge-v") || semver::Version::parse(&tag[8..]).is_err() {
        return Err("invalid release tag".into());
    }
    let mut result = Err("no release identity".to_string());
    for repository in RELEASE_REPOSITORIES {
        let identity = format!(
            "https://github.com/{repository}/.github/workflows/release.yml@refs/tags/{tag}"
        );
        result = verify_signature_with_identity(cosign, sums, bundle, &identity, COSIGN_ISSUER);
        if result.is_ok() {
            break;
        }
    }
    result
}

fn verify_signature_with_identity(
    cosign: &Path,
    sums: &Path,
    bundle: &Path,
    identity: &str,
    issuer: &str,
) -> Result<(), String> {
    let mut child = Command::new(cosign)
        .arg("verify-blob")
        .arg("--bundle")
        .arg(bundle)
        .arg("--certificate-identity")
        .arg(identity)
        .arg("--certificate-oidc-issuer")
        .arg(issuer)
        .arg(sums)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run pinned cosign: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) if status.success() => return Ok(()),
            Some(_) => return Err("release signature rejected by pinned cosign".into()),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("release signature verification timed out".into());
            }
            None => thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn verify_archive(sums: &[u8], archive: &[u8], name: &str) -> Result<(), String> {
    let text = std::str::from_utf8(sums).map_err(|_| "invalid SHA256SUMS encoding")?;
    let matching: Vec<&str> = text
        .lines()
        .filter_map(|line| {
            let (hash, file) = line.split_once("  ")?;
            (file == name).then_some(hash)
        })
        .collect();
    if matching.len() != 1
        || matching[0].len() != 64
        || !matching[0].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("signed checksums do not contain one valid archive digest".into());
    }
    if digest(archive) != matching[0].to_ascii_lowercase() {
        return Err("release archive digest mismatch".into());
    }
    Ok(())
}

fn extract_binary(archive: &[u8], destination: &Path) -> Result<(), String> {
    let decoder = flate2::read::GzDecoder::new(Cursor::new(archive));
    let mut archive = tar::Archive::new(decoder);
    let mut entries = archive.entries().map_err(|e| e.to_string())?;
    let Some(entry) = entries.next() else {
        return Err("release archive is empty".into());
    };
    let mut entry = entry.map_err(|e| e.to_string())?;
    if entry.path().map_err(|e| e.to_string())?.as_ref() != Path::new("build-bridge")
        || !entry.header().entry_type().is_file()
        || entry.size() > MAX_BINARY
    {
        return Err("release archive must contain one regular build-bridge".into());
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    drop(entry);
    if entries.next().is_some() {
        return Err("release archive contains extra entries".into());
    }
    write_private(destination, &bytes, 0o700)
}

/// Download all three release assets, verify signer and archive, and stage the binary.
pub async fn stage(
    client: &Client,
    release: &Release,
    cache: &Path,
    destination: &Path,
) -> Result<(), String> {
    let version = release
        .tag
        .strip_prefix("bridge-v")
        .ok_or("invalid bridge tag")?;
    if release.version != version || semver::Version::parse(version).is_err() {
        return Err("release version and tag disagree".into());
    }
    let repo = release_repo()?;
    let base = format!(
        "https://github.com/{repo}/releases/download/{}",
        release.tag
    );
    let cosign = verified_cosign(client, cache).await?;
    stage_from_base(client, release, destination, &base, &cosign).await
}

async fn stage_from_base(
    client: &Client,
    release: &Release,
    destination: &Path,
    base: &str,
    cosign: &Path,
) -> Result<(), String> {
    stage_from_base_with_verifier(client, release, destination, base, |sums, bundle, tag| {
        verify_signature(cosign, sums, bundle, tag)
    })
    .await
}

async fn stage_from_base_with_verifier<F>(
    client: &Client,
    release: &Release,
    destination: &Path,
    base: &str,
    verify: F,
) -> Result<(), String>
where
    F: FnOnce(&Path, &Path, &str) -> Result<(), String>,
{
    let archive_name = format!("build-bridge-{}.tar.gz", platform_key()?);
    let archive = download(client, &format!("{base}/{archive_name}"), MAX_ARCHIVE).await?;
    let sums = download(client, &format!("{base}/SHA256SUMS"), MAX_MANIFEST).await?;
    let bundle = download(
        client,
        &format!("{base}/SHA256SUMS.sigstore.json"),
        MAX_MANIFEST,
    )
    .await?;
    let work = destination.parent().ok_or("staged binary has no parent")?;
    let sums_path = work.join("SHA256SUMS");
    let bundle_path = work.join("SHA256SUMS.sigstore.json");
    write_private(&sums_path, &sums, 0o600)?;
    write_private(&bundle_path, &bundle, 0o600)?;
    verify(&sums_path, &bundle_path, &release.tag)?;
    verify_archive(&sums, &archive, &archive_name)?;
    extract_binary(&archive, destination)
}

#[cfg(test)]
mod signature_tests;

pub struct ProductionBackend {
    client: Client,
    home: PathBuf,
    tasks_dir: PathBuf,
    installed_binary: PathBuf,
}

impl ProductionBackend {
    pub fn new(home: PathBuf, tasks_dir: PathBuf, installed_binary: PathBuf) -> Self {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .expect("update HTTP client");
        Self {
            client,
            home,
            tasks_dir,
            installed_binary,
        }
    }
}

#[async_trait]
impl UpdateBackend for ProductionBackend {
    async fn latest(&self) -> Result<Release, String> {
        latest(&self.client).await
    }

    async fn install(&self, release: &Release, attempt_id: &str) -> Result<(), String> {
        self.stage(release, attempt_id).await?;
        self.launch(release, attempt_id).await
    }

    async fn stage(&self, release: &Release, attempt_id: &str) -> Result<(), String> {
        if super::installer::probation_active(&self.home) {
            return Err("another update is active or recovering".into());
        }
        let installed = super::provenance::managed_binary(&self.home, &self.installed_binary)?;
        let (dir, job) = super::installer::create_job(
            &self.home,
            &self.tasks_dir,
            &installed,
            &release.version,
            attempt_id,
        )?;
        let work = job
            .staged_binary
            .parent()
            .ok_or("update job has no directory")?;
        for name in [
            "staged-build-bridge",
            "SHA256SUMS",
            "SHA256SUMS.sigstore.json",
        ] {
            let path = work.join(name);
            if path.exists() {
                fs::remove_file(path).map_err(|e| e.to_string())?;
            }
        }
        let cache = super::installer::updates_dir(&self.home).join("tools");
        stage(&self.client, release, &cache, &job.staged_binary).await?;
        super::installer::record_staged(&dir)
    }

    async fn launch(&self, release: &Release, attempt_id: &str) -> Result<(), String> {
        if super::installer::probation_active(&self.home) {
            return Err("another update is active or recovering".into());
        }
        let dir = super::installer::updates_dir(&self.home)
            .join("jobs")
            .join(attempt_id);
        let job: super::installer::Job =
            serde_json::from_slice(&fs::read(dir.join("job.json")).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if job.nonce != attempt_id || job.version != release.version || !job.staged_binary.is_file()
        {
            return Err("staged update does not match this attempt".into());
        }
        super::installer::launch(&dir)
    }

    fn active_attempt(&self) -> Result<Option<String>, String> {
        super::installer::active_attempt(&self.home)
    }

    fn maintain(&self) -> Result<(), String> {
        super::installer::ensure_recovery(&self.home)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_digest_must_match_exact_archive() {
        let name = "build-bridge-linux-x86_64.tar.gz";
        let archive = b"archive";
        let sums = format!("{}  {name}\n", digest(archive));
        assert!(verify_archive(sums.as_bytes(), archive, name).is_ok());
        assert!(verify_archive(sums.as_bytes(), b"tampered", name).is_err());
        assert!(verify_archive(format!("{sums}{sums}").as_bytes(), archive, name).is_err());
    }

    #[test]
    fn archive_rejects_traversal_and_extra_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "build-bridge", &b"good"[..])
            .unwrap();
        let bytes = tar.into_inner().unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&bytes).unwrap();
        let archive = encoder.finish().unwrap();
        assert!(extract_binary(&archive, &dir.path().join("binary")).is_ok());
    }
}
