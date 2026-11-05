# Build Secure Transport JS Library

Browser-native ESM helpers for the client side of the `E2E-MVP.md` protocol.

### Sealed secret uploads

`sealSecretUpload({ deviceTransportPublicKeyB64, context, value })` seals a
version-1 `build.secrets.upload` body to the device transport key supplied by
authenticated device discovery. `context` contains `device_id`, `creator`, a
canonical UUID `request_id`, `name`, nullable `ttl_seconds`, `grant` and `stage`.
`grant` is `{ scope: 'project', project_id }` or
`{ scope: 'agent', agent_id, conversation_id, generation }`; `stage` contains
`owner_id`, `conversation_id`, `generation` and nullable `draft_id`.

The plaintext frame is exactly 32 KiB: a four-byte big-endian JSON length,
UTF-8 JSON (with the exact UTF-8 value encoded as `value_b64`, standard base64
without padding), then zero padding. Encoding the value separately keeps all
16 KiB values within the frame even when JSON escaping would expand control
characters. The libsodium sealed-box ciphertext is base64
without padding. Rust uses dryoc to authenticate/open it and compares every
context field with the authenticated outer request. The full create request
is capped at 64 KiB. Names match `[A-Za-z_][A-Za-z0-9_.-]{0,63}`; values are
nonempty UTF-8 of at most 16 KiB without NUL; TTL is null or 60–2592000 seconds.

Values and sealed requests must remain ephemeral: do not save either in a
generic write-intent queue, draft cache or logs. After an uncertain create,
reconcile using the metadata request ID. Retrying a live create uses the exact
same sealed bytes; a new seal needs a new request ID.
