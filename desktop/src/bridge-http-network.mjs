import { createParser } from 'eventsource-parser';

export const MAX_JSON_BYTES = 1024 * 1024;
export function transportError(code) {
  return Object.assign(new Error(`Bridge HTTP ${code.replaceAll('_', ' ')}.`), {code});
}
const refuse = code => { throw transportError(code); };

export function canonicalOrigin(value) {
  if (typeof value !== 'string' || value.length > 2048 || value.trim() !== value || /[\\\s]/u.test(value)) refuse('invalid_origin');
  let url;
  try { url = new URL(value); } catch { refuse('invalid_origin'); }
  if (url.protocol !== 'https:' || url.username || url.password || url.search || url.hash) refuse('invalid_origin');
  // Compare the original path as well: URL normalizes away dot segments.
  const authority = value.slice(value.indexOf('://') + 3).replace(/\/$/u, '');
  if (authority.includes('/') || authority.includes('?') || authority.includes('#') || url.port === '0') refuse('invalid_origin');
  if (url.hostname.endsWith('.')) refuse('invalid_origin');
  const rawHost = authority.replace(/:\d+$/u, '').toLowerCase();
  if (/^[\d.]+$/u.test(url.hostname) && rawHost !== url.hostname) refuse('invalid_origin');
  return url.origin;
}

export async function abortable(promise, signal) {
  if (signal?.aborted) throw transportError('canceled');
  let cancel;
  const aborted = new Promise((_, reject) => {
    cancel=()=>reject(transportError('canceled'));
    signal?.addEventListener('abort',cancel,{once:true});
  });
  try {return await Promise.race([promise,aborted]);}
  finally {signal?.removeEventListener('abort',cancel);}
}

export async function boundedJson(response, maxBytes, signal) {
  const reader = response.body?.getReader();
  if (!reader) refuse('invalid_response');
  let size = 0;
  const parts = [];
  try {
    while (true) {
      const {done, value} = await abortable(reader.read(), signal);
      if (done) break;
      size += value.byteLength;
      if (size > maxBytes) refuse('response_too_large');
      parts.push(value);
    }
    const buffer = Buffer.concat(parts, size);
    try { return JSON.parse(new TextDecoder('utf-8', {fatal:true}).decode(buffer)); }
    catch { refuse('invalid_response'); }
  } finally {
    void reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

export function networkOptions({method = 'GET', body, headers = {}, signal, maxBody = MAX_JSON_BYTES} = {}) {
  const encoded = body === undefined ? undefined : JSON.stringify(body);
  if (encoded && Buffer.byteLength(encoded) > maxBody) refuse('request_too_large');
  return {
    method, body: encoded, signal, headers: {'Accept':'application/json', ...(encoded === undefined ? {} : {'Content-Type':'application/json'}), ...headers},
    credentials:'omit', redirect:'error', cache:'no-store', referrerPolicy:'no-referrer',
  };
}

export async function jsonRequest(fetch, url, options = {}) {
  const controller = new AbortController();
  const signal = options.signal ? AbortSignal.any([options.signal, controller.signal]) : controller.signal;
  const timer = setTimeout(() => controller.abort(), options.timeoutMs ?? 10_000);
  try {
    const response = await abortable(fetch(url, networkOptions({...options, signal})), signal);
    if (response.status === 401) {void response.body?.cancel();refuse('unauthorized');}
    if (!response.ok && !options.apiEnvelope) {void response.body?.cancel();refuse('http_error');}
    if (response.status === 204) return null;
    const value=await boundedJson(response, options.maxBytes ?? MAX_JSON_BYTES, signal);
    if (!response.ok && (typeof value?.error !== 'string' || typeof value.error_code !== 'string' || typeof value.retryable !== 'boolean')) refuse('http_error');
    return value;
  } catch (error) {
    if (options.signal?.aborted) refuse('canceled');
    if (controller.signal.aborted) refuse('timeout');
    if (error.code && error.message?.startsWith('Bridge HTTP ')) throw error;
    refuse(controller.signal.aborted ? 'timeout' : 'network_error');
  } finally { clearTimeout(timer); }
}

const EVENTS = new Set(['hello', 'changes', 'operation', 'event', 'reset_required', 'closing']);
export async function consumeEvents(response, onEvent, {signal, idleMs = 30_000} = {}) {
  if (!response.ok || response.headers.get('content-type')?.split(';')[0] !== 'text/event-stream') refuse('invalid_stream');
  const reader = response.body.getReader();
  const decoder = new TextDecoder('utf-8', {fatal:true});
  const parser = createParser({
    maxBufferSize: MAX_JSON_BYTES,
    onError: error => refuse(error.type === 'max-buffer-size-exceeded' ? 'stream_too_large' : 'invalid_stream'),
    onId: () => refuse('invalid_stream'),
    onEvent: ({event, data}) => {
      if (!EVENTS.has(event) || Buffer.byteLength(data) > MAX_JSON_BYTES) refuse('invalid_stream');
      let parsed;
      try { parsed = JSON.parse(data); } catch { refuse('invalid_stream'); }
      onEvent({event, data:parsed});
    },
  });
  const cancel = () => { void reader.cancel().catch(() => {}); };
  signal?.addEventListener('abort', cancel, {once:true});
  let timer;
  try {
    while (!signal?.aborted) {
      let timeout;
      const deadline = new Promise((_, reject) => { timer = setTimeout(() => { cancel(); reject(transportError('stream_timeout')); }, idleMs); });
      try { timeout = await Promise.race([reader.read(), deadline]); } finally { clearTimeout(timer); }
      if (timeout.done) break;
      parser.feed(decoder.decode(timeout.value, {stream:true}));
    }
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', cancel);
    void reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
