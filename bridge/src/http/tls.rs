//! Native TLS configuration, validated before any listener bind.
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use sha2::{Digest, Sha256};

const MAX_PEM_BYTES: u64 = 65_536;

pub struct LoadedTls {
    pub config: Arc<ServerConfig>,
    pub certificate_fingerprint: String,
}

pub fn load(cert_path: &Path, key_path: &Path) -> Result<Arc<ServerConfig>, String> {
    load_identity(cert_path, key_path).map(|loaded| loaded.config)
}

/// Read each configured file exactly once so the persisted rotation identity
/// is the certificate installed in this exact server configuration.
pub fn load_identity(cert_path: &Path, key_path: &Path) -> Result<LoadedTls, String> {
    let cert_bytes = read_pem(cert_path, false)?;
    let key_bytes = read_pem(key_path, true)?;
    let certificates = CertificateDer::pem_slice_iter(&cert_bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "invalid HTTP TLS certificate chain")?;
    if certificates.is_empty() {
        return Err("HTTP TLS certificate chain is empty".into());
    }
    let certificate_fingerprint = hex(&Sha256::digest(certificates[0].as_ref()));
    let key = PrivateKeyDer::from_pem_slice(&key_bytes)
        .map_err(|_| "invalid or missing HTTP TLS private key")?;
    let mut config = ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| "HTTP TLS protocol configuration failed")?
    .with_no_client_auth()
    .with_single_cert(certificates, key)
    .map_err(|_| "HTTP TLS certificate and key are incompatible")?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(LoadedTls {
        config: Arc::new(config),
        certificate_fingerprint,
    })
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn read_pem(path: &Path, private: bool) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|_| "cannot open HTTP TLS file")?;
    let metadata = file
        .metadata()
        .map_err(|_| "cannot inspect HTTP TLS file")?;
    if !metadata.is_file() || metadata.len() > MAX_PEM_BYTES {
        return Err("HTTP TLS file must be a regular file of at most 64KiB".into());
    }
    if private {
        check_private_permissions(&metadata)?;
    }
    let mut bytes = Vec::new();
    file.take(MAX_PEM_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read HTTP TLS file")?;
    if bytes.len() as u64 > MAX_PEM_BYTES {
        return Err("HTTP TLS file exceeds 64KiB".into());
    }
    Ok(bytes)
}

#[cfg(unix)]
fn check_private_permissions(metadata: &std::fs::Metadata) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no arguments and only reads process credentials.
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(
            "HTTP TLS private key must be owned by this user with owner-only permissions".into(),
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_private_permissions(_: &std::fs::Metadata) -> Result<(), String> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "HTTP TLS key permissions require a supported owner-only ACL validator",
    )
    .to_string())
}
