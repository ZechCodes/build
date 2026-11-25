//! Release pin for the optional hosted tunnel connector.

use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use super::{
    download, release_repo, verified_cosign, verify_archive, verify_signature, write_private,
    Client, MAX_MANIFEST,
};
use std::fs::{self, OpenOptions};
use std::io::Read;

const VERSION: &str = "2026.11.2";
const PINNED_SUMS: &[u8] = include_bytes!("../../../cloudflared.SHA256SUMS");
const MAX_CONNECTOR: usize = 96 * 1024 * 1024;

#[cfg(target_arch = "x86_64")]
const ARCH: &str = "amd64";
#[cfg(target_arch = "aarch64")]
const ARCH: &str = "arm64";

fn asset_name(arch: &str) -> String {
    format!("cloudflared-{VERSION}-linux-{arch}")
}

pub(crate) async fn install(
    directory: PathBuf,
    override_path: Option<PathBuf>,
) -> Result<PathBuf, String> {
    initialize_directory(&directory)?;
    let client = Client::builder()
        .https_only(true)
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|_| "cannot prepare connector downloader")?;
    let sums = signed_manifest(&client, &directory).await?;
    let name = asset_name(ARCH);
    let destination = directory.join(&name);
    let executable = override_path.as_ref().unwrap_or(&destination);
    if executable.exists() || override_path.is_some() {
        let executable = executable.clone();
        return tokio::task::spawn_blocking(move || {
            let managed = executable == destination;
            let bytes = read_executable(&executable, managed)?;
            verify_pin(&sums, &bytes, &name)?;
            if !managed {
                install_atomic(&destination, &bytes)?;
            }
            Ok(destination)
        })
        .await
        .map_err(|_| "connector verification worker failed")?;
    }

    let source = format!(
        "https://github.com/cloudflare/cloudflared/releases/download/{VERSION}/cloudflared-linux-{ARCH}"
    );
    let bytes = download(&client, &source, MAX_CONNECTOR).await?;
    tokio::task::spawn_blocking(move || {
        verify_pin(&sums, &bytes, &name)?;
        install_atomic(&destination, &bytes)?;
        Ok(destination)
    })
    .await
    .map_err(|_| "connector installation worker failed")?
}

fn initialize_directory(directory: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    let parent = directory.parent().ok_or("missing connector install root")?;
    for path in [parent, directory] {
        match builder.create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("cannot create connector install directory".into()),
        }
        validate_directory(path)?;
    }
    Ok(())
}

fn validate_directory(directory: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(directory)
        .map_err(|_| "cannot inspect connector install directory")?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || directory.canonicalize().ok().as_deref() != Some(directory)
    {
        return Err("connector install directory must be private and unaliased".into());
    }
    Ok(())
}

async fn signed_manifest(client: &Client, dir: &Path) -> Result<Vec<u8>, String> {
    let tag = format!("bridge-v{}", env!("CARGO_PKG_VERSION"));
    let base = format!(
        "https://github.com/{}/releases/download/{tag}",
        release_repo()?
    );
    let cosign = verified_cosign(client, dir).await?;
    let sums_path = dir.join("SHA256SUMS");
    let bundle_path = dir.join("SHA256SUMS.sigstore.json");
    if sums_path.is_file() && bundle_path.is_file() {
        let saved = verify_manifest(
            cosign.clone(),
            sums_path.clone(),
            bundle_path.clone(),
            tag.clone(),
        )
        .await;
        if saved.is_ok() {
            return saved;
        }
    }
    let sums = download(client, &format!("{base}/SHA256SUMS"), MAX_MANIFEST).await?;
    let bundle = download(
        client,
        &format!("{base}/SHA256SUMS.sigstore.json"),
        MAX_MANIFEST,
    )
    .await?;
    install_file(&sums_path, &sums, 0o600)?;
    install_file(&bundle_path, &bundle, 0o600)?;
    verify_manifest(cosign, sums_path, bundle_path, tag).await
}

async fn verify_manifest(
    cosign: PathBuf,
    sums: PathBuf,
    bundle: PathBuf,
    tag: String,
) -> Result<Vec<u8>, String> {
    tokio::task::spawn_blocking(move || {
        let bytes = read_regular(&sums, MAX_MANIFEST)?;
        read_regular(&bundle, MAX_MANIFEST)?;
        verify_signature(&cosign, &sums, &bundle, &tag)?;
        Ok(bytes)
    })
    .await
    .map_err(|_| "connector manifest verification worker failed")?
}

fn read_executable(path: &Path, managed: bool) -> Result<Vec<u8>, String> {
    if !path.is_absolute() || path.canonicalize().ok().as_deref() != Some(path) {
        return Err("connector executable must be an absolute, unaliased path".into());
    }
    let (file, metadata) = open_regular(path)?;
    if metadata.mode() & 0o111 == 0 || (managed && !private_executable(&metadata)) {
        return Err("connector executable has unsafe ownership or permissions".into());
    }
    read_bounded(file, MAX_CONNECTOR)
}

fn private_executable(metadata: &fs::Metadata) -> bool {
    metadata.uid() == rustix::process::geteuid().as_raw()
        && metadata.mode() & 0o7777 == 0o700
        && metadata.nlink() == 1
}

fn read_regular(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let (file, _) = open_regular(path)?;
    read_bounded(file, limit)
}

fn open_regular(path: &Path) -> Result<(fs::File, fs::Metadata), String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "cannot open connector asset")?;
    let metadata = file
        .metadata()
        .map_err(|_| "cannot inspect connector asset")?;
    if !metadata.is_file() {
        return Err("connector asset has an unsafe file type or mode".into());
    }
    Ok((file, metadata))
}

fn read_bounded(file: fs::File, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read connector asset")?;
    if bytes.len() > limit {
        return Err("connector asset exceeds size bound".into());
    }
    Ok(bytes)
}

fn verify_pin(signed: &[u8], bytes: &[u8], name: &str) -> Result<(), String> {
    verify_archive(PINNED_SUMS, bytes, name)?;
    verify_archive(signed, bytes, name)
}

fn install_atomic(destination: &Path, bytes: &[u8]) -> Result<(), String> {
    install_file(destination, bytes, 0o700)
}

fn install_file(destination: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let temporary = destination.with_extension(format!("part-{}", uuid::Uuid::new_v4()));
    let result = write_private(&temporary, bytes, mode).and_then(|()| {
        fs::rename(&temporary, destination).map_err(|_| "cannot publish connector asset".into())
    });
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_connector_rejects_wrong_platform_and_tampered_payload() {
        let name = asset_name(ARCH);
        let other_platform = asset_name(if ARCH == "amd64" { "arm64" } else { "amd64" });
        let bytes = b"tampered executable";
        let attacker = format!("{}  {name}\n", super::super::digest(bytes));
        assert!(verify_pin(attacker.as_bytes(), bytes, &name).is_err());
        assert!(verify_archive(PINNED_SUMS, bytes, &other_platform).is_err());
        assert!(verify_archive(
            format!("{}  {other_platform}\n", super::super::digest(bytes)).as_bytes(),
            bytes,
            &name
        )
        .is_err());
    }
}
