//! Private local release composition. Value reads never enter RPC or MCP dispatch.
mod transport;
use super::runtime::sessions::SecretReleaseAuthority;
use super::AppState;
use crate::secrets::{Result, SecretError, SecretService, SecretValue};
use std::path::Path;
use std::sync::{Arc, Mutex};

struct ReleaseBackend {
    authority: SecretReleaseAuthority,
    service: Option<SecretService>,
}
impl transport::Backend for ReleaseBackend {
    fn release(&self, request: &transport::Request) -> Result<SecretValue> {
        let caller = self
            .authority
            .authenticate(&request.agent_id, &request.capability);
        let Some(caller) = caller else {
            if let Some(service) = &self.service {
                service.record_unauthenticated_failure();
            }
            return Err(SecretError::Unauthorized);
        };
        let service = self.service.as_ref().ok_or(SecretError::Unavailable)?;
        let result = service.release(&caller, &request.secret_id, &request.request_id, || {
            self.authority.revalidate(&caller)
        });
        if matches!(&result, Err(SecretError::Unauthorized)) {
            service.record_unauthenticated_failure();
        }
        result
    }
}

impl AppState {
    pub fn spawn_secret_release_socket(state: Arc<Mutex<Self>>, control_path: String) {
        tokio::spawn(async move {
            let backend = super::off_the_workers(move || {
                let app = state.lock().unwrap();
                Arc::new(ReleaseBackend {
                    authority: app.secret_release_authority(),
                    service: app.secret_service(),
                })
            })
            .await;
            let path = format!("{control_path}.secrets");
            let listener = match bind_private_listener(Path::new(&path)).await {
                Ok(listener) => listener,
                Err(_) => {
                    eprintln!("secret release socket unavailable");
                    return;
                }
            };
            transport::serve(listener, backend).await;
        });
    }
}

async fn bind_private_listener(path: &Path) -> std::io::Result<tokio::net::UnixListener> {
    use std::io::ErrorKind;
    // A full accept backlog must not park a runtime worker or be mistaken for
    // stale debris. A timed-out probe always refuses replacement.
    match tokio::time::timeout(
        std::time::Duration::from_millis(250),
        tokio::net::UnixStream::connect(path),
    )
    .await
    {
        Ok(Ok(_)) | Err(_) => {
            return Err(std::io::Error::new(
                ErrorKind::AddrInUse,
                "secret socket already served",
            ))
        }
        Ok(Err(error))
            if matches!(
                error.kind(),
                ErrorKind::NotFound | ErrorKind::ConnectionRefused
            ) => {}
        Ok(Err(error)) => return Err(error),
    }
    let path = path.to_owned();
    let listener = super::off_the_workers(move || bind_socket_file(&path)).await?;
    tokio::net::UnixListener::from_std(listener)
}

fn bind_socket_file(path: &Path) -> std::io::Result<std::os::unix::net::UnixListener> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "unsafe secret socket entry",
                ));
            }
            std::fs::remove_file(path)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    let listener = std::os::unix::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    #[tokio::test]
    async fn full_backlog_probe_is_bounded_and_preserves_live_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic-backlog.secrets");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        use std::os::fd::AsRawFd;
        // SAFETY: the listener fd is live; listen changes only its backlog.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
        let mut clients = Vec::new();
        // More attempts than the explicit backlog, with no accept, exercise
        // the full queue even on kernels reporting pending connects as ready.
        for _ in 0..8 {
            match tokio::time::timeout(
                std::time::Duration::from_millis(2),
                tokio::net::UnixStream::connect(&path),
            )
            .await
            {
                Ok(Ok(client)) => clients.push(client),
                Err(_) => break,
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Ok(Err(error)) => panic!("synthetic backlog: {error}"),
            }
        }
        assert!(!clients.is_empty());
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            bind_private_listener(&path),
        )
        .await
        .unwrap();
        assert!(result.is_err(), "a full live socket cannot be replaced");
        assert!(path.exists());
        drop((clients, listener));
    }

    #[tokio::test]
    async fn listener_is_private_and_never_replaces_live_socket_or_other_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.secrets");
        let first = bind_private_listener(&path).await.unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(bind_private_listener(&path).await.is_err());
        drop(first);
        let stale = bind_private_listener(&path).await.unwrap();
        drop(stale);
        std::fs::remove_file(&path).unwrap();
        let file = dir.path().join("synthetic-file");
        std::fs::write(&file, b"synthetic-data").unwrap();
        symlink(&file, &path).unwrap();
        assert!(bind_private_listener(&path).await.is_err());
        assert!(std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"synthetic-file").unwrap();
        assert!(bind_private_listener(&path).await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"synthetic-file");
    }
}
