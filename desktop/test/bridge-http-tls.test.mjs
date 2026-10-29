import assert from "node:assert/strict";
import test from "node:test";
import { createServer } from "node:https";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";

// Real sockets and Node's normal certificate verifier; trusting a throwaway CA
// in a child process avoids changing the test runner or desktop TLS policy.
test("native HTTPS validates certificates and rejects credential redirects on real sockets", async (t) => {
  const directory = mkdtempSync(join(tmpdir(), "desktop-http-tls-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const key = join(directory, "key.pem");
  const cert = join(directory, "cert.pem");
  execFileSync(
    "openssl",
    [
      "req",
      "-x509",
      "-newkey",
      "rsa:2048",
      "-nodes",
      "-keyout",
      key,
      "-out",
      cert,
      "-days",
      "1",
      "-subj",
      "/CN=localhost",
      "-addext",
      "subjectAltName=DNS:localhost",
    ],
    { stdio: "ignore" },
  );
  const received = [];
  const server = createServer(
    { key: readFileSync(key), cert: readFileSync(cert) },
    (request, response) => {
      received.push({
        path: request.url,
        authorization: request.headers.authorization,
      });
      if (request.url === "/redirect") {
        response.writeHead(302, { location: "/stolen" });
        response.end();
        return;
      }
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify({ ok: true }));
    },
  );
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  t.after(() => {
    server.closeAllConnections();
    server.close();
  });
  const port = server.address().port;
  const module = new URL("../src/bridge-http-network.mjs", import.meta.url)
    .href;
  async function run(source, trusted = true) {
    const env = { ...process.env };
    delete env.NODE_EXTRA_CA_CERTS;
    if (trusted) env.NODE_EXTRA_CA_CERTS = cert;
    const child = spawn(
      process.execPath,
      [
        "--input-type=module",
        "-e",
        `import assert from 'node:assert/strict';import {jsonRequest} from ${JSON.stringify(module)};${source}`,
      ],
      { env, stdio: ["ignore", "pipe", "pipe"] },
    );
    let stderr = "";
    child.stderr.on("data", (data) => (stderr += data));
    const [code] = await once(child, "close");
    assert.equal(code, 0, stderr);
  }
  await run(
    `assert.deepEqual(await jsonRequest(fetch,'https://localhost:${port}/ok',{headers:{Authorization:'Bearer fixture-device-token'}}),{ok:true});`,
  );
  await run(
    `await assert.rejects(jsonRequest(fetch,'https://localhost:${port}/redirect',{headers:{Authorization:'Bearer fixture-device-token'}}),{code:'network_error'});`,
  );
  await run(
    `await assert.rejects(jsonRequest(fetch,'https://localhost:${port}/untrusted'),{code:'network_error'});`,
    false,
  );
  await run(
    `await assert.rejects(jsonRequest(fetch,'https://127.0.0.1:${port}/wrong-host'),{code:'network_error'});`,
  );
  assert.deepEqual(received, [
    { path: "/ok", authorization: "Bearer fixture-device-token" },
    { path: "/redirect", authorization: "Bearer fixture-device-token" },
  ]);
});
