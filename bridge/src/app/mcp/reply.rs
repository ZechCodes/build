//! The existing MCP response emission sequence and its native origin outcome.
use serde_json::Value;
use tokio::io::{AsyncWrite, AsyncWriteExt};

#[cfg(test)]
mod tests;

pub(super) async fn emit<W: AsyncWrite + Unpin>(
    writer: &mut W,
    response: Value,
    #[cfg(test)] origin: Option<&crate::timing::write_origin::WriteOrigin>,
) {
    let bytes = response.to_string();
    #[cfg(test)]
    let emission = origin.map(|origin| origin.begin_reply());
    // Preserve all three existing operations even when an earlier one fails.
    let _content_written = writer.write_all(bytes.as_bytes()).await;
    let _newline_written = writer.write_all(b"\n").await;
    let _flushed = writer.flush().await;
    #[cfg(test)]
    if let Some(emission) = emission {
        emission.finish(_content_written.is_ok() && _newline_written.is_ok() && _flushed.is_ok());
    }
}
