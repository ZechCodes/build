//! Environment inputs for the optional native HTTPS listener.
//!
//! This stage reads no files and opens no sockets. Origin canonicalization and
//! paired-issuer validation belong to the identity module before runtime start.
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Clone)]
pub struct HttpSettings {
    pub bind: SocketAddr,
    pub origin: String,
    pub tls_cert: PathBuf,
    pub tls_key: PathBuf,
    pub allowed_origins: Option<Vec<String>>,
}

impl HttpSettings {
    pub fn resolve(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, String> {
        let enabled = match lookup("BRIDGE_HTTP_ENABLED").as_deref() {
            None | Some("false" | "0") => false,
            Some("true" | "1") => true,
            _ => return Err("BRIDGE_HTTP_ENABLED must be true, false, 1 or 0".into()),
        };
        if !enabled {
            return Ok(None);
        }
        let bind = required(&lookup, "BRIDGE_HTTP_BIND")?
            .parse::<SocketAddr>()
            .map_err(|_| "BRIDGE_HTTP_BIND must be an explicit IP:port")?;
        if bind.port() == 0 {
            return Err("BRIDGE_HTTP_BIND requires a nonzero port".into());
        }
        Ok(Some(Self {
            bind,
            origin: required(&lookup, "BRIDGE_HTTP_ORIGIN")?,
            tls_cert: required(&lookup, "BRIDGE_HTTP_TLS_CERT")?.into(),
            tls_key: required(&lookup, "BRIDGE_HTTP_TLS_KEY")?.into(),
            allowed_origins: allowed_origins(&lookup)?,
        }))
    }
}

fn required(lookup: &impl Fn(&str) -> Option<String>, name: &str) -> Result<String, String> {
    lookup(name)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{name} is required when HTTP is enabled"))
}

fn allowed_origins(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Option<Vec<String>>, String> {
    let Some(raw) = lookup("BRIDGE_HTTP_ALLOWED_ORIGINS") else {
        return Ok(None);
    };
    let origins: Vec<_> = raw
        .split(',')
        .map(|value| value.trim().to_string())
        .collect();
    if origins.len() > 16
        || origins
            .iter()
            .any(|value| value.is_empty() || value == "*" || value == "null")
    {
        return Err("BRIDGE_HTTP_ALLOWED_ORIGINS requires 1..16 exact HTTPS origins".into());
    }
    Ok(Some(origins))
}
