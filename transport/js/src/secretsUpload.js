// Versioned secret upload plaintext is sealed before any generic RPC adapter.
const PADDED_BYTES = 32 * 1024;
const MAX_VALUE_BYTES = 16 * 1024;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

function invalid() { throw new Error('secret_invalid'); }

function objectFields(value, fields) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) invalid();
  const keys = Object.keys(value);
  if (keys.length !== fields.length || keys.some(key => !fields.includes(key))) invalid();
}

function text(value) {
  if (typeof value !== 'string' || !value || new TextEncoder().encode(value).length > 128 || value.includes('\0')) invalid();
}

function validateGrant(grant) {
  const fields = grant?.scope === 'project'
    ? ['scope', 'project_id'] : ['scope', 'agent_id', 'conversation_id', 'generation'];
  objectFields(grant, fields);
  if (!['project', 'agent'].includes(grant.scope)) invalid();
  for (const key of fields) text(grant[key]);
}

function validateStage(stage) {
  objectFields(stage, ['owner_id', 'conversation_id', 'generation', 'draft_id']);
  for (const key of ['owner_id', 'conversation_id', 'generation']) text(stage[key]);
  if (stage.draft_id !== null) text(stage.draft_id);
}

function validateContext(context) {
  objectFields(context, ['device_id', 'creator', 'request_id', 'name', 'ttl_seconds', 'grant', 'stage']);
  text(context.device_id);
  text(context.creator);
  if (!UUID.test(context.request_id) || !/^[A-Za-z_][A-Za-z0-9_.-]{0,63}$/.test(context.name)) invalid();
  if (context.ttl_seconds !== null && (!Number.isInteger(context.ttl_seconds) || context.ttl_seconds < 60 || context.ttl_seconds > 2592000)) invalid();
  validateGrant(context.grant);
  validateStage(context.stage);
}

export function sealSecretUploadWithSodium(sodium, { deviceTransportPublicKeyB64, context, value }) {
  validateContext(context);
  if (typeof value !== 'string') invalid();
  const valueBytes = new TextEncoder().encode(value);
  try {
    if (!valueBytes.length || valueBytes.length > MAX_VALUE_BYTES || value.includes('\0')) invalid();
  } finally {
    sodium.memzero(valueBytes);
  }
  if (typeof deviceTransportPublicKeyB64 !== 'string' || deviceTransportPublicKeyB64.length !== 43) invalid();
  const recipient = sodium.from_base64(deviceTransportPublicKeyB64, sodium.base64_variants.ORIGINAL_NO_PADDING);
  if (recipient.length !== 32) invalid();
  const encoded = sodium.from_string(JSON.stringify({ domain: 'build.secrets.upload', version: 1, context, value }));
  const padded = new Uint8Array(PADDED_BYTES);
  try {
    if (encoded.length > PADDED_BYTES - 4) invalid();
    new DataView(padded.buffer).setUint32(0, encoded.length);
    padded.set(encoded, 4);
    return sodium.to_base64(sodium.crypto_box_seal(padded, recipient), sodium.base64_variants.ORIGINAL_NO_PADDING);
  } finally {
    sodium.memzero(encoded);
    sodium.memzero(padded);
  }
}
