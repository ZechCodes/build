import { createWireBudget } from "./bridge-http-budget.mjs";
import { createBridgeEventStreams } from "./bridge-http-events.mjs";
import { randomUUID } from "node:crypto";
import {
  canonicalOrigin,
  jsonRequest,
  MAX_JSON_BYTES,
} from "./bridge-http-network.mjs";
import {
  HOSTED_ORIGIN,
  fail,
  exact,
  identifier,
  segment,
  verifiedMint,
  requestOptions,
  buildRoute,
  compatibleTransport,
} from "./bridge-http-contract.mjs";

// Only main constructs owners, chooses the manifest and supplies authenticated
// account state. No renderer-controlled callback can create verified authority.
export function createBridgeHttp({
  fetch = globalThis.fetch,
  getAuth,
  isOwnerCurrent,
  send,
  manifest,
}) {
  const handles = new Map();
  const pending = new Map();

  let epoch = 0;
  const budget = createWireBudget();
  const families = new Map();

  function capture(owner) {
    const auth = getAuth();
    if (!auth?.accessToken || !isOwnerCurrent(owner)) fail("stale_owner");
    return { owner, generation: auth.generation, epoch };
  }
  function currentAuthority(binding) {
    const auth = getAuth();
    return (
      binding.epoch === epoch &&
      binding.generation === auth?.generation &&
      Boolean(auth?.accessToken) &&
      isOwnerCurrent(binding.owner)
    );
  }
  function check(binding) {
    if (!currentAuthority(binding)) fail("stale_owner");
    if (binding.controller?.signal.aborted) fail("canceled");
    const handle = binding.handle || binding;
    if (
      performance.now() >=
      Math.min(handle.deadline ?? Infinity, handle.tokenDeadline ?? Infinity)
    ) {
      retire(handle);
      fail("unauthorized");
    }
  }
  function handleFor(owner, id) {
    const handle = handles.get(id);
    if (!handle || handle.owner !== owner) fail("invalid_handle");
    check(handle);
    if (handle.mint.expires_at * 1000 <= Date.now()) {
      retire(handle);
      fail("unauthorized");
    }
    return handle;
  }
  async function tracked(
    binding,
    callback,
    requestId = randomUUID(),
    allowance,
  ) {
    check(binding);
    if (
      pending.size >= 64 ||
      [...pending.values()].filter((value) => value.handle === binding)
        .length >= 32
    )
      fail("busy");
    const key = `${binding.owner.id ?? ""}:${requestId}`;
    if (pending.has(key)) fail("duplicate_request");
    const release = budget.reserve(allowance);
    const operation = {
      ...binding,
      handle: binding,
      requestId,
      controller: new AbortController(),
    };
    pending.set(key, operation);

    try {
      const result = await callback(operation.controller.signal);
      check(operation);
      return result;
    } finally {
      release();
      if (pending.get(key) === operation) pending.delete(key);
    }
  }
  const hosted = (path, body, binding, method = "POST") =>
    tracked(binding, (signal) =>
      jsonRequest(fetch, `${HOSTED_ORIGIN}${path}`, {
        method,
        body,
        signal,
        headers: { Authorization: `Bearer ${getAuth().accessToken}` },
        maxBytes: 32 * 1024,
      }),
    );
  const headers = (handle) => ({
    Authorization: `Bearer ${handle.mint.access_token}`,
    ...(handle.opening ? { "X-Build-Session": handle.opening.session_id } : {}),
  });
  const events = createBridgeEventStreams({
    fetch,
    check,
    headers,
    send,
    device,
    handleFor,
    retire,
  });
  async function device(handle, path, options = {}) {
    try {
      return await tracked(
        handle,
        (signal) =>
          jsonRequest(fetch, `${handle.mint.origin}${path}`, {
            ...options,
            signal,
            headers: { ...headers(handle), ...options.headers },
          }),
        options.requestId,
        {
          requestMax: options.maxBody ?? MAX_JSON_BYTES,
          responseMax: options.maxBytes ?? MAX_JSON_BYTES,
          opening: handle,
        },
      );
    } catch (error) {
      if (error.code === "unauthorized") retire(handle);
      throw error;
    }
  }
  function metadata(handle) {
    const mint = handle.mint;
    return {
      handle: handle.id,
      deviceId: mint.device_id,
      origin: mint.origin,
      endpointEpoch: mint.endpoint_epoch,
      familyId: mint.family_id,
    };
  }
  function adopt(binding, mint) {
    check(binding);
    if (
      [...handles.values()].filter((value) => value.owner === binding.owner)
        .length >= 16
    )
      fail("busy");
    if (families.size >= 64 && !families.has(mint.family_id)) fail("busy");
    families.set(mint.family_id, {
      device: mint.device_id,
      family: mint.family_id,
    });
    const handle = { ...binding, id: randomUUID(), mint, streams: new Set() };
    handle.tokenDeadline =
      performance.now() + mint.expires_at * 1000 - Date.now();
    handles.set(handle.id, handle);
    handle.expiry = setTimeout(
      () => retire(handle),
      Math.max(0, mint.expires_at * 1000 - Date.now()),
    );
    handle.expiry.unref?.();
    return metadata(handle);
  }
  async function discover(owner, input) {
    const binding = capture(owner);
    const origin = canonicalOrigin(input);
    const challenge = await hosted(
      "/api/bridge-discovery/challenges",
      { origin },
      binding,
    );
    identifier(challenge.challenge_id, "invalid_response");
    if (
      challenge.issuer !== HOSTED_ORIGIN ||
      typeof challenge.nonce !== "string" ||
      challenge.nonce.length < 32 ||
      challenge.nonce.length > 128 ||
      challenge.expires_at * 1000 <= Date.now()
    )
      fail("invalid_response");
    const proof = await tracked(binding, (signal) =>
      jsonRequest(fetch, `${origin}/.well-known/build-bridge`, {
        method: "POST",
        body: { challenge_id: challenge.challenge_id, nonce: challenge.nonce },
        signal,
        maxBytes: 16 * 1024,
        timeoutMs: 5000,
      }),
    );
    if (typeof proof.proof !== "string" || proof.proof.length > 16000)
      fail("invalid_response");
    const result = await hosted(
      "/api/bridge-discovery/complete",
      { challenge_id: challenge.challenge_id, proof: proof.proof },
      binding,
    );
    return adopt(binding, verifiedMint(result, { origin }));
  }
  async function resume(owner, hint) {
    exact(hint, ["deviceId", "origin", "endpointEpoch", "familyId"]);
    const binding = capture(owner);
    const expected = {
      device_id: identifier(hint.deviceId),
      family_id: identifier(hint.familyId),
      endpoint_epoch: identifier(hint.endpointEpoch),
    };
    if (hint.origin !== undefined)
      expected.origin = canonicalOrigin(hint.origin);
    const result = await hosted(
      `/api/devices/${segment(hint.deviceId)}/http-credentials`,
      { family_id: hint.familyId, endpoint_epoch: hint.endpointEpoch },
      binding,
    );
    return adopt(binding, verifiedMint(result, expected));
  }
  function installOpening(handle, opening, start, previous) {
    compatibleTransport(manifest, opening.transport);
    identifier(opening.greeting?.bridge_epoch, "invalid_response");
    identifier(opening.session_id, "invalid_response");
    identifier(opening.opening_generation, "invalid_response");
    if (
      previous &&
      (opening.session_id !== previous.session_id ||
        opening.opening_generation !== previous.opening_generation)
    )
      fail("binding_mismatch");
    const deadline = Math.min(
      Date.parse(opening.grant_expires_at),
      handle.mint.expires_at * 1000,
      Date.now() + Math.max(0, 30000 - (performance.now() - start)),
    );
    if (!Number.isFinite(deadline) || deadline <= Date.now())
      fail("invalid_response");
    clearTimeout(handle.hard);
    clearTimeout(handle.refresh);
    handle.opening = opening;
    handle.deadline = performance.now() + deadline - Date.now();
    handle.hard = setTimeout(() => retire(handle), deadline - Date.now());
    handle.hard.unref?.();
    handle.refresh = setTimeout(
      () => void renew(handle).catch(() => retire(handle)),
      Math.max(0, deadline - Date.now() - 10000),
    );
    handle.refresh.unref?.();
    return opening;
  }
  async function open(owner, id, apiRange) {
    const handle = handleFor(owner, id);
    if (typeof apiRange !== "string" || apiRange.length > 128 || !apiRange)
      fail("invalid_params");
    if (handle.opening || handle.openingPending) fail("already_open");
    handle.openingPending = true;
    try {
      const start = performance.now();
      return installOpening(
        handle,
        await device(handle, "/v1/sessions", {
          method: "POST",
          body: { api_range: apiRange },
        }),
        start,
      );
    } finally {
      handle.openingPending = false;
    }
  }
  async function renew(handle) {
    check(handle);
    if (handle.mint.expires_at * 1000 - Date.now() < 60000) {
      const mint = handle.mint;
      const updated = await hosted(
        `/api/devices/${segment(mint.device_id)}/http-credentials`,
        { family_id: mint.family_id, endpoint_epoch: mint.endpoint_epoch },
        handle,
      );
      const expected = Object.fromEntries(
        [
          "account_id",
          "device_id",
          "origin",
          "endpoint_epoch",
          "family_id",
          "issuer",
          "scope",
        ].map((key) => [key, mint[key]]),
      );
      handle.mint = verifiedMint(updated, expected);
      handle.tokenDeadline =
        performance.now() + handle.mint.expires_at * 1000 - Date.now();
      clearTimeout(handle.expiry);
      handle.expiry = setTimeout(
        () => retire(handle),
        handle.mint.expires_at * 1000 - Date.now(),
      );
      handle.expiry.unref?.();
    }
    const previous = handle.opening;
    const start = performance.now();
    installOpening(
      handle,
      await device(handle, `/v1/sessions/${segment(previous.session_id)}`, {
        method: "PATCH",
      }),
      start,
      previous,
    );
  }
  async function request(owner, id, operationId, params = {}, options = {}) {
    const handle = handleFor(owner, id);
    if (!handle.opening) fail("not_open");
    const route = buildRoute(manifest, operationId, params);
    const extra = requestOptions(options);
    Object.assign(extra, events.requestHeader(handle, route.requiresEvents));
    let dispatched = false;
    try {
      return await device(handle, route.path, {
        ...route,
        headers: extra,
        requestId: options.requestId,
        apiEnvelope: true,
        onDispatch: () => {
          dispatched = true;
        },
      });
    } catch (error) {
      if (dispatched) error.uncertain = true;
      throw error;
    }
  }
  function cancel(owner, requestId) {
    identifier(requestId);
    for (const operation of pending.values())
      if (operation.owner === owner && operation.requestId === requestId)
        operation.controller.abort();
  }
  function retire(handle) {
    if (!handles.delete(handle.id)) return;
    clearTimeout(handle.hard);
    clearTimeout(handle.refresh);
    clearTimeout(handle.expiry);
    events.clear(handle, "credentials_closed");
    for (const operation of pending.values())
      if (operation.handle === handle) operation.controller.abort();
    handle.mint.access_token = "";
  }
  async function close(owner, id) {
    const handle = handleFor(owner, id);
    const session = handle.opening?.session_id;
    const authHeaders = headers(handle);
    retire(handle);
    if (session)
      await jsonRequest(
        fetch,
        `${handle.mint.origin}/v1/sessions/${segment(session)}`,
        { method: "DELETE", headers: authHeaders, timeoutMs: 5000 },
      );
  }
  async function revoke(accessToken) {
    const trackedFamilies = [...families.values()];
    families.clear();
    clear();
    const results = await Promise.allSettled(
      trackedFamilies.map(({ device, family }) =>
        jsonRequest(
          fetch,
          `${HOSTED_ORIGIN}/api/devices/${segment(device)}/http-credentials/${segment(family)}`,
          {
            method: "DELETE",
            headers: { Authorization: `Bearer ${accessToken}` },
            timeoutMs: 2000,
            maxBytes: 1024,
          },
        ),
      ),
    );
    return {
      failed: results.filter((result) => result.status === "rejected").length,
    };
  }
  function clear(owner) {
    if (!owner) epoch++;
    for (const handle of handles.values())
      if (!owner || handle.owner === owner) retire(handle);
    for (const operation of pending.values())
      if (!owner || operation.owner === owner) operation.controller.abort();
  }
  return {
    discover,
    resume,
    open,
    request,
    subscribe: events.subscribe,
    ack: events.ack,
    unsubscribe: events.unsubscribe,
    cancel,
    close,
    clear,
    revoke,
  };
}
