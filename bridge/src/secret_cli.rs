//! Raw retrieval using only the dedicated launch context and private socket.
use crate::secrets::MAX_VALUE_BYTES;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use zeroize::{Zeroize, Zeroizing};

type Result<T> = std::result::Result<T, &'static str>;

struct Context {
    socket: PathBuf,
    agent: String,
    token: Zeroizing<String>,
    secret_id: String,
}

impl Context {
    fn resolve(args: &[String]) -> Result<Self> {
        let (id, requested_env) = arguments(args)?;
        let channel = required("BRIDGE_SECRET_CHANNEL")?;
        if !matches!(channel.as_str(), "dev" | "prod")
            || requested_env.is_some_and(|env| env != channel)
        {
            return Err("unauthorized");
        }
        let socket = PathBuf::from(required("BRIDGE_SECRET_SOCKET")?);
        if !socket.is_absolute() {
            return Err("unauthorized");
        }
        required("BRIDGE_SECRET_OWNER")?;
        Ok(Self {
            socket,
            agent: required("BRIDGE_SECRET_AGENT")?,
            token: Zeroizing::new(required("BRIDGE_SECRET_TOKEN")?),
            secret_id: id.into(),
        })
    }
}

fn required(key: &str) -> Result<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.is_empty() && value.len() <= 128 && !value.contains('\0'))
        .ok_or("unauthorized")
}

fn arguments(args: &[String]) -> Result<(&str, Option<&str>)> {
    if args.first().map(String::as_str) != Some("get") {
        return Err("secret_invalid");
    }
    let mut id = None;
    let mut environment = None;
    let (pairs, remainder) = args[1..].as_chunks::<2>();
    for pair in pairs {
        match pair[0].as_str() {
            "--id" if id.is_none() => id = Some(pair[1].as_str()),
            "--env" if environment.is_none() => environment = Some(pair[1].as_str()),
            _ => return Err("secret_invalid"),
        }
    }
    if !remainder.is_empty() {
        return Err("secret_invalid");
    }
    let id = id.ok_or("secret_invalid")?;
    crate::secrets::validate_id(id).map_err(|_| "secret_invalid")?;
    Ok((id, environment))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    ok: bool,
    length: Option<usize>,
    error: Option<String>,
}

async fn retrieve(context: Context) -> Result<Zeroizing<Vec<u8>>> {
    let mut stream = tokio::net::UnixStream::connect(context.socket)
        .await
        .map_err(|_| "unauthorized")?;
    #[derive(Serialize)]
    struct Request<'a> {
        agent_id: &'a str,
        capability: &'a str,
        secret_id: &'a str,
        request_id: String,
    }
    let mut request = Zeroizing::new(
        serde_json::to_vec(&Request {
            agent_id: &context.agent,
            capability: &context.token,
            secret_id: &context.secret_id,
            request_id: uuid::Uuid::new_v4().to_string(),
        })
        .map_err(|_| "secret_unavailable")?,
    );
    request.push(b'\n');
    stream
        .write_all(&request)
        .await
        .map_err(|_| "secret_unavailable")?;
    request.zeroize();
    read_response(stream).await
}

async fn read_response(stream: tokio::net::UnixStream) -> Result<Zeroizing<Vec<u8>>> {
    let mut reader = tokio::io::BufReader::new(stream);
    let mut header = Zeroizing::new(Vec::new());
    (&mut reader)
        .take(257)
        .read_until(b'\n', &mut header)
        .await
        .map_err(|_| "secret_unavailable")?;
    if header.len() > 256 || header.last() != Some(&b'\n') {
        return Err("secret_unavailable");
    }
    let response: Response = serde_json::from_slice(&header).map_err(|_| "secret_unavailable")?;
    if !response.ok {
        return Err(safe_error(response.error.as_deref()));
    }
    let length = response
        .length
        .filter(|length| *length <= MAX_VALUE_BYTES)
        .filter(|_| response.error.is_none())
        .ok_or("secret_unavailable")?;
    let mut value = Zeroizing::new(vec![0; length]);
    reader
        .read_exact(&mut value)
        .await
        .map_err(|_| "secret_unavailable")?;
    Ok(value)
}

fn safe_error(code: Option<&str>) -> &'static str {
    match code {
        Some("unauthorized") => "unauthorized",
        Some("secret_revoked") => "secret_revoked",
        Some("secret_expired") => "secret_expired",
        Some("secret_limit") => "secret_limit",
        Some("project_archived") => "project_archived",
        _ => "secret_unavailable",
    }
}

/// No configuration, vault paths, identity keys or working-directory discovery.
pub async fn run(args: &[String]) -> i32 {
    let result = match Context::resolve(args) {
        Ok(context) => tokio::time::timeout(Duration::from_secs(6), retrieve(context))
            .await
            .unwrap_or(Err("secret_unavailable")),
        Err(error) => Err(error),
    };
    let result = result.and_then(|value| {
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(&value)
            .and_then(|_| stdout.flush())
            .map_err(|_| "secret_unavailable")
    });
    if let Err(error) = result {
        let _ = writeln!(std::io::stderr().lock(), "{error}");
        return 1;
    }
    0
}
