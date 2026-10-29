import { canonicalBridgeOrigin, httpFailure } from "./httpEndpoint.js";
import { boundedJson, httpRequest, jsonHeaders } from "./httpFetch.js";

const nonempty = (value) => typeof value === "string" && value.length > 0 && value.length <= 512;
const alive = (value, now) => Number.isFinite(value) && value > now() / 1000;
const validPending = (pending, accountId, issuer, now) => pending && pending.account_id === accountId
  && pending.issuer === issuer && alive(pending.expires_at, now) && nonempty(pending.challenge_id) && nonempty(pending.nonce);
const validMint = (mint, accountId, issuer, origin, now) => mint && mint.issuer === issuer
  && mint.account_id === accountId && mint.origin === origin && alive(mint.expires_at, now);
const validMintFields = (mint) => [mint.device_id, mint.endpoint_epoch, mint.family_id, mint.access_token].every(nonempty)
  && !/[\r\n]/.test(mint.access_token) && typeof mint.scope === "string" && mint.scope.split(" ").includes("bridge:read");
const sameBinding = (mint, identity) => mint.device_id === identity.deviceId && mint.origin === identity.origin
  && mint.endpoint_epoch === identity.endpointEpoch && mint.family_id === identity.familyId;

export function createBridgeDiscovery({ issuer, accountId, fetchImpl = fetch, isCurrent = () => true, now = Date.now }) {
  if (new URL(issuer).origin !== issuer || !nonempty(accountId)) throw httpFailure("authentication");
  const hosted = (path, body, method = "POST") => httpRequest(async (signal) => {
    const response = await fetchImpl(issuer + path, {
      method, credentials: "same-origin", redirect: "error", referrerPolicy: "no-referrer", signal,
      headers: jsonHeaders, ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    if (!response.ok) throw httpFailure("authentication");
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
    if (!validMint(mint, accountId, issuer, origin, now)) {
      throw httpFailure("authentication");
    }
    if (!validMintFields(mint)) throw httpFailure("authentication");
    if (identity && !sameBinding(mint, identity)) throw httpFailure("authentication");
    return mint;
  };

  const renew = async (identity) => verifyMint(await hosted(`/api/devices/${encodeURIComponent(identity.deviceId)}/http-credentials`, {
    family_id: identity.familyId, endpoint_epoch: identity.endpointEpoch,
  }), identity.origin, identity);

  const hold = (initial) => {
    let mint = initial;
    let closed = false;
    let renewing = null;
    const identity = Object.freeze({
      deviceId: mint.device_id, origin: mint.origin, endpointEpoch: mint.endpoint_epoch, familyId: mint.family_id,
      name: typeof mint.name === "string" ? mint.name : "", fingerprint: typeof mint.fingerprint === "string" ? mint.fingerprint : "",
    });
    const active = () => !closed && isCurrent();
    const check = () => { if (!active() || !alive(mint.expires_at, now)) throw httpFailure("authentication"); };
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
      if (!nonempty(challengeId)) throw httpFailure("protocol");
      const pending = verifyPending(await hosted(`/api/bridge-discovery/challenges/${encodeURIComponent(challengeId)}`, undefined, "GET"));
      if (pending.challenge_id !== challengeId) throw httpFailure("authentication");
      const { proof } = await httpRequest(async (signal) => {
        const response = await fetchImpl(pending.origin + "/.well-known/build-bridge", {
          method: "POST", credentials: "omit", redirect: "error", referrerPolicy: "no-referrer", signal, headers: jsonHeaders,
          body: JSON.stringify({ challenge_id: pending.challenge_id, nonce: pending.nonce }),
        });
        if (!response.ok) throw httpFailure("authentication");
        return boundedJson(response, 16 * 1024);
      }, { isCurrent, timeoutMs: 5000 });
      if (typeof proof !== "string" || proof.length > 16 * 1024) throw httpFailure("protocol");
      const mint = verifyMint(await hosted("/api/bridge-discovery/complete", { challenge_id: challengeId, proof }), pending.origin);
      return hold(mint);
    },
    async resume(identity) {
      canonicalBridgeOrigin(identity.origin);
      if (![identity.deviceId, identity.endpointEpoch, identity.familyId].every(nonempty)) throw httpFailure("authentication");
      return hold(await renew(identity));
    },
  };
}
