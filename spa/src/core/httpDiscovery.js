import { canonicalBridgeOrigin, httpFailure } from "./httpEndpoint.js";
import { boundedJson, httpRequest, jsonHeaders } from "./httpFetch.js";

const nonempty = (value) => typeof value === "string" && value.length > 0 && value.length <= 512;
const alive = (value, now) => Number.isFinite(value) && value > now() / 1000;
const validPending = (pending, accountId, issuer, now) => pending && pending.account_id === accountId
  && pending.issuer === issuer && alive(pending.expires_at, now) && nonempty(pending.challenge_id) && nonempty(pending.nonce);
const validMint = (mint, accountId, issuer, origin) => mint && mint.issuer === issuer
  && mint.account_id === accountId && mint.origin === origin && Number.isFinite(mint.expires_at);
const validMintFields = (mint) => [mint.device_id, mint.endpoint_epoch, mint.family_id, mint.access_token].every(nonempty)
  && !/[\r\n]/.test(mint.access_token) && hasClientScopes(mint.scope);
const hasClientScopes = (scope) => typeof scope === "string" && ["bridge:read", "bridge:write"].every((required) => scope.split(" ").includes(required));
const sameBinding = (mint, identity) => mint.device_id === identity.deviceId && mint.origin === identity.origin
  && mint.endpoint_epoch === identity.endpointEpoch && mint.family_id === identity.familyId;

export function createBridgeDiscovery({ issuer, accountId, fetchImpl = fetch, isCurrent = () => true, now = Date.now }) {
  if (new URL(issuer).origin !== issuer || !nonempty(accountId)) throw httpFailure("authentication");
  const hosted = (path, body, method = "POST") => httpRequest(async (signal) => {
    const response = await fetchImpl(issuer + path, {
      method, credentials: "same-origin", redirect: "error", referrerPolicy: "no-referrer", signal,
      headers: jsonHeaders, ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    if ([408, 429].includes(response.status) || response.status >= 500) throw httpFailure("network");
    if (!response.ok) {
      const code = response.status === 409 ? "wrong_bridge" : "authentication";
      throw Object.assign(httpFailure(code), { status: response.status });
    }
    return boundedJson(response, 16 * 1024);
  }, { isCurrent, timeoutMs: 5000 });

  const verifyPending = (pending, expectedOrigin) => {
    if (!validPending(pending, accountId, issuer, now)) throw httpFailure("authentication");
    if (canonicalBridgeOrigin(pending.origin) !== pending.origin || expectedOrigin && pending.origin !== expectedOrigin) throw httpFailure("authentication");
    const expected = `/app/?bridge_discovery=${encodeURIComponent(pending.challenge_id)}`;
    if (pending.resume_url !== expected) throw httpFailure("protocol");
    return pending;
  };

  const verifyMint = (mint, origin, identity) => {
    if (!validMint(mint, accountId, issuer, origin)) {
      throw httpFailure("authentication");
    }
    if (!validMintFields(mint)) throw httpFailure("authentication");
    if (identity && !sameBinding(mint, identity)) throw httpFailure("authentication");
    if (identity && !matchesSavedDevice(mint, identity)) throw httpFailure("wrong_bridge");
    if (!alive(mint.expires_at, now)) throw httpFailure("scope_ended");
    return mint;
  };

  const renew = async (identity) => verifyMint(await hosted(`/api/devices/${encodeURIComponent(identity.deviceId)}/http-credentials`, {
    family_id: identity.familyId, endpoint_epoch: identity.endpointEpoch,
  }), identity.origin, identity);

  const verifySaved = (pending, identity) => {
    verifyPending(pending, identity.origin);
    if (pending.device_id !== identity.deviceId) throw httpFailure("wrong_bridge");
    return pending;
  };

  const completePending = async (pending, expected) => {
    const { challenge_id: challengeId, nonce } = pending;
    const { proof } = await httpRequest(async (signal) => {
      let response;
      try {
        response = await fetchImpl(pending.origin + "/.well-known/build-bridge", {
          method: "POST", credentials: "omit", redirect: "error", referrerPolicy: "no-referrer", signal, headers: jsonHeaders,
          body: JSON.stringify({ challenge_id: challengeId, nonce }),
        });
      } catch { throw httpFailure("network"); }
      if (!response.ok) throw httpFailure("discovery_failed");
      return boundedJson(response, 16 * 1024);
    }, { isCurrent, timeoutMs: 5000 });
    if (typeof proof !== "string" || proof.length > 16 * 1024) throw httpFailure("protocol");
    const mint = verifyMint(await hosted("/api/bridge-discovery/complete", { challenge_id: challengeId, proof }), pending.origin);
    if (expected && !matchesSavedDevice(mint, expected)) throw httpFailure("wrong_bridge");
    return hold(mint);
  };

  const readPending = async (challengeId) => {
    if (!nonempty(challengeId)) throw httpFailure("protocol");
    const pending = verifyPending(await hosted(`/api/bridge-discovery/challenges/${encodeURIComponent(challengeId)}`, undefined, "GET"));
    if (pending.challenge_id !== challengeId) throw httpFailure("authentication");
    return pending;
  };

  const hold = (initial) => {
    let mint = initial;
    let closed = false;
    let renewing = null;
    const identity = Object.freeze({
      deviceId: mint.device_id, origin: mint.origin, endpointEpoch: mint.endpoint_epoch, familyId: mint.family_id,
      name: typeof mint.name === "string" ? mint.name : "", fingerprint: typeof mint.fingerprint === "string" ? mint.fingerprint : "",
    });
    const active = () => !closed && isCurrent();
    const check = () => { if (!active() || !alive(mint.expires_at, now)) throw httpFailure("scope_ended"); };
    return {
      identity,
      token() { check(); return mint.access_token; },
      expiresAt: () => mint.expires_at * 1000,
      renew() {
        check();
        renewing ||= renew(identity).then((next) => {
          if (!active()) throw httpFailure("scope_ended");
          mint = next;
        }).finally(() => { renewing = null; });
        return renewing;
      },
      close() { closed = true; mint = { expires_at: 0, access_token: "" }; },
    };
  };

  return {
    async begin(input) {
      const origin = canonicalBridgeOrigin(input);
      const pending = verifyPending(await hosted("/api/bridge-discovery/challenges", { origin }), origin);
      return issuer + pending.resume_url;
    },
    async complete(challengeId) {
      const pending = await readPending(challengeId);
      if (pending.device_id) throw httpFailure("wrong_bridge");
      return completePending(pending);
    },
    async beginSaved(identity) {
      const origin = canonicalBridgeOrigin(identity.origin);
      if (!nonempty(identity.deviceId)) throw httpFailure("protocol");
      const path = `/api/devices/${encodeURIComponent(identity.deviceId)}/http-discovery`;
      const pending = await hosted(path, { origin }).catch((error) => {
        if ([401, 403].includes(error.status)) throw httpFailure("sign_in");
        throw error;
      });
      return verifySaved(pending, identity);
    },
    finishSaved(pending, identity) {
      return completePending(verifySaved(pending, identity), identity);
    },
    async resumeSavedChallenge(challengeId, identity) {
      return completePending(verifySaved(await readPending(challengeId), identity), identity);
    },
    async resume(identity) {
      canonicalBridgeOrigin(identity.origin);
      if (![identity.deviceId, identity.endpointEpoch, identity.familyId].every(nonempty)) throw httpFailure("authentication");
      return hold(await renew(identity));
    },
  };
}

function matchesSavedDevice(mint, identity) {
  return mint.device_id === identity.deviceId && (!identity.fingerprint || mint.fingerprint === identity.fingerprint);
}
