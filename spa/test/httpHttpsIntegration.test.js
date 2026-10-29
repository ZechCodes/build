import { describe, expect, it } from "vitest";
import { createServer } from "node:https";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const execute = promisify(execFile);

describe("real HTTPS REST/SSE client witness", () => {
  it("keeps TLS verification, binds credentials, cleans lost stream openings and preserves uncertain writes", async () => {
    const directory = await mkdtemp(join(tmpdir(), "build-http-912-"));
    let server;
    try {
      await execute("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-subj", "/CN=localhost", "-addext", "subjectAltName=IP:127.0.0.1,DNS:localhost", "-keyout", join(directory, "key.pem"), "-out", join(directory, "cert.pem")]);
      const sessions = new Map(); const requests = [];
      let next = 0; let effects = 0; let origin; let subscriptionAttempts = 0;
      const greeting = { api_version: "3.24.0", capabilities: ["bridge.epoch"], bridge_epoch: "epoch", journal_incarnation: "journal" };
      const pending = () => ({ origin, issuer: origin, account_id: "account", challenge_id: "challenge", nonce: "nonce", expires_at: Date.now() / 1000 + 60, resume_url: "/app/?bridge_discovery=challenge" });
      server = createServer({ key: await readFile(join(directory, "key.pem")), cert: await readFile(join(directory, "cert.pem")) }, async (request, response) => {
        const chunks = []; for await (const chunk of request) chunks.push(chunk);
        const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : {};
        requests.push({ path: request.url, method: request.method, headers: request.headers, body });
        const json = (value, status = 200) => { response.writeHead(status, { "content-type": "application/json" }); response.end(JSON.stringify(value)); };
        if (request.url.startsWith("/api/bridge-discovery/challenges")) return json(pending());
        if (request.url === "/.well-known/build-bridge") return json({ proof: "fixture-proof" });
        if (request.url === "/api/bridge-discovery/complete") return json({ issuer: origin, origin, account_id: "account", device_id: "device", endpoint_epoch: "ep", family_id: "fam", access_token: "fixture-device-bearer", expires_at: Date.now() / 1000 + 300, scope: "bridge:read bridge:write" });
        if (request.headers.authorization !== "Bearer fixture-device-bearer") return json({ error: "auth", error_code: "unauthorized" }, 401);
        if (request.url === "/v1/sessions" && request.method === "POST") {
          const id = `session-${++next}`;
          const opening = { session_id: id, opening_generation: id, greeting, grant_expires_at: new Date(Date.now() + 30000).toISOString(), transport: { resource_manifest_version: 1, operation_ids: ["ping", "update"], terminal: "webrtc", durable_events: false } };
          sessions.set(id, { opening }); return json(opening, 201);
        }
        if (request.url.startsWith("/v1/sessions/") && request.method === "DELETE") {
          const id = request.url.split("/").at(-1); sessions.get(id)?.stream?.end(); sessions.delete(id); response.writeHead(204); return response.end();
        }
        const held = sessions.get(request.headers["x-build-session"]);
        if (!held) return json({ error: "closed", error_code: "scope_ended" }, 409);
        if (request.url === "/v1/subscriptions") {
          held.binding = `stream-${held.opening.session_id}`;
          subscriptionAttempts += 1;
          if (subscriptionAttempts === 2) { response.destroy(); return; }
          return json({ subscription_id: held.binding, opening_generation: held.opening.opening_generation, events_path: `/v1/subscriptions/${held.binding}/events` }, 201);
        }
        if (request.url.endsWith("/events")) {
          held.stream = response;
          response.writeHead(200, { "content-type": "text/event-stream" });
          return response.write(`event: hello\ndata: ${JSON.stringify({ subscription_id: held.binding, opening_generation: held.opening.opening_generation, bridge_epoch: "epoch", durable: false, recovery: "current_state_refresh" })}\n\n`);
        }
        if (request.method === "DELETE" && request.url.startsWith("/v1/subscriptions/")) { held.stream?.end(); response.writeHead(204); return response.end(); }
        if (request.headers["x-build-subscription"] !== held.binding) return json({ error: "binding", error_code: "stream_replaced" }, 409);
        if (request.url === "/v1/tasks/t") {
          effects += 1;
          if (body.lose) { response.destroy(); return; }
          held.stream.write(`event: changes\ndata: ${JSON.stringify({ type: "changes", bridge_epoch: "epoch", items: [] })}\n\n`);
        }
        return json({ ok: true, result: { effects }, bridge_epoch: "epoch" });
      });
      await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
      origin = `https://127.0.0.1:${server.address().port}`;
      const witness = new URL("./fixtures/httpClientWitness.mjs", import.meta.url).pathname;
      const trusted = await execute(process.execPath, [witness, origin, "trusted"], { env: { ...process.env, NODE_EXTRA_CA_CERTS: join(directory, "cert.pem") }, timeout: 15000 });
      expect(trusted.stdout).toContain("trusted HTTPS witness passed");
      expect(effects).toBe(2); // One successful write, one uncertain write, no replay.
      expect(sessions.size).toBe(0);
      const directDiscovery = requests.find((request) => request.path === "/.well-known/build-bridge");
      expect(directDiscovery.headers.authorization).toBeUndefined();
      expect(directDiscovery.headers.cookie).toBeUndefined();
      expect(requests.filter((request) => request.path === "/v1/subscriptions")).toHaveLength(4);
      const before = requests.length;
      const untrusted = await execute(process.execPath, [witness, origin, "untrusted"], { env: { ...process.env, NODE_EXTRA_CA_CERTS: "" }, timeout: 10000 });
      expect(untrusted.stdout).toContain("untrusted TLS refused");
      expect(requests).toHaveLength(before);
    } finally {
      server?.closeAllConnections();
      if (server) await new Promise((resolve) => server.close(resolve));
      await rm(directory, { recursive: true, force: true });
    }
  }, 30000);
});
