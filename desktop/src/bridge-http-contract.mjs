import { canonicalOrigin, transportError, MAX_JSON_BYTES } from './bridge-http-network.mjs';

export const HOSTED_ORIGIN = 'https://getbuild.ing';
export const fail = code => { throw transportError(code); };
export function object(value, code='invalid_params') {
  if (!value || typeof value !== 'object' || Array.isArray(value)) fail(code);
  return value;
}
export function exact(value, keys, code='invalid_params') {
  object(value, code);
  if (Object.keys(value).some(key=>!keys.includes(key))) fail(code);
  return value;
}
export function identifier(value, code='invalid_params') {
  if (typeof value !== 'string' || !value.length || value.length > 128 || /[\x00-\x20\x7f]/u.test(value) || value === '.' || value === '..') fail(code);
  return value;
}
export const segment = value => encodeURIComponent(identifier(value));

export function verifiedMint(value, expected={}) {
  object(value, 'invalid_response');
  for (const name of ['account_id','device_id','endpoint_epoch','family_id']) identifier(value[name], 'invalid_response');
  if (value.issuer !== HOSTED_ORIGIN || canonicalOrigin(value.origin) !== value.origin) fail('binding_mismatch');
  for (const [name, expectedValue] of Object.entries(expected)) if (value[name] !== expectedValue) fail('binding_mismatch');
  if (typeof value.access_token !== 'string' || !/^[\x21-\x7e]{16,4096}$/u.test(value.access_token)) fail('invalid_response');
  if (!Number.isFinite(value.expires_at) || value.expires_at*1000 <= Date.now() || value.expires_at*1000 > Date.now()+301000) fail('invalid_response');
  if (value.scope !== 'bridge:read bridge:write' && value.scope !== 'bridge:read') fail('invalid_response');
  return value;
}

export function requestOptions(options = {}) {
  exact(options, ['requestId','priority','write'], 'invalid_options');
  if (options.priority !== undefined && !['foreground','background'].includes(options.priority)) fail('invalid_options');
  const headers = {'X-Build-Priority':options.priority === 'background' ? 'background' : 'user'};
  if (options.requestId !== undefined) headers['X-Build-Request-Id'] = identifier(options.requestId, 'invalid_options');
  if (options.write !== undefined) {
    exact(options.write, ['operation_id','after'], 'invalid_options');
    headers['Idempotency-Key'] = identifier(options.write.operation_id, 'invalid_options');
    if (options.write.after !== undefined) {
      const after=JSON.stringify(options.write.after);
      if (Buffer.byteLength(after) > 8192) fail('invalid_options');
      headers['X-Build-After'] = after;
    }
  }
  return headers;
}

export function buildRoute(manifest, operationId, params) {
  object(params);
  const route = manifest?.operations?.find(value=>value.operationId === operationId);
  if (!route) fail('unsupported_operation');
  if (!['GET','POST','PUT','PATCH','DELETE'].includes(route.verb) || !route.path.startsWith('/v1/') || route.path.includes('?')) fail('invalid_manifest');
  if (!Number.isSafeInteger(route.responseMaxBytes) || route.responseMaxBytes <= 0 || route.responseMaxBytes > 64*MAX_JSON_BYTES) fail('invalid_manifest');
  let path=route.path;
  const body={...params};
  for (const key of route.pathParams) { path=path.replaceAll(`{${key}}`,segment(params[key])); delete body[key]; }
  if (/[{}]/u.test(path) || new URL(path,HOSTED_ORIGIN).origin !== HOSTED_ORIGIN) fail('invalid_manifest');
  return {path,method:route.verb,body:route.body === 'remaining' ? body : undefined,maxBytes:route.responseMaxBytes};
}
