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

test("real native adapter discovers, opens, streams and calls an explicit resource over HTTPS", async (t) => {
  const directory = mkdtempSync(join(tmpdir(), "desktop-adapter-tls-"));
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
  const requests = [];
  const manifestUrl = new URL(
    "../src/bridge-http-resources.json",
    import.meta.url,
  );
  const resourceManifest = JSON.parse(readFileSync(manifestUrl));
  const opening = {
    session_id: "session",
    opening_generation: "generation",
    grant_expires_at: new Date(Date.now() + 30000).toISOString(),
    greeting: { bridge_epoch: "epoch" },
    transport: resourceManifest.transport,
  };
  const server = createServer(
    { key: readFileSync(key), cert: readFileSync(cert) },
    async (request, response) => {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      requests.push({
        url: request.url,
        method: request.method,
        headers: request.headers,
        body: Buffer.concat(chunks).toString(),
      });
      const json = (value) => {
        response.writeHead(200, { "content-type": "application/json" });
        response.end(JSON.stringify(value));
      };
      if (request.url === "/.well-known/build-bridge")
        return json({ proof: "fixture-proof" });
      if (request.url === "/v1/sessions") return json(opening);
      if (request.url === "/v1/subscriptions")
        return json({
          subscription_id: "subscription",
          opening_generation: "generation",
          events_path: "/v1/subscriptions/subscription/events",
        });
      if (request.url === "/v1/subscriptions/subscription/events") {
        response.writeHead(200, { "content-type": "text/event-stream" });
        response.write(
          "event: hello\ndata: " +
            JSON.stringify({
              subscription_id: "subscription",
              opening_generation: "generation",
              bridge_epoch: "epoch",
              durable: false,
              recovery: "current_state_refresh",
            }) +
            "\n\n",
        );
        response.write(
          'event: changes\ndata: {"method":"changes","params":{"project_id":"project"}}\n\n',
        );
        return;
      }
      if (request.url === "/v1/projects")
        return json({
          id: "request",
          ok: true,
          result: { projects: ["project"] },
          bridge_epoch: "epoch",
        });
      if (request.method === "DELETE") {
        response.writeHead(204);
        response.end();
        return;
      }
      response.writeHead(404);
      response.end();
    },
  );
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  t.after(() => {
    server.closeAllConnections();
    server.close();
  });
  const origin = `https://localhost:${server.address().port}`;
  const module = new URL("../src/bridge-http.mjs", import.meta.url).href;
  const source = `
    import assert from 'node:assert/strict';
    import {createBridgeHttp} from ${JSON.stringify(module)};
    import resourceManifest from ${JSON.stringify(manifestUrl.href)} with {type:"json"};
    const origin=${JSON.stringify(origin)};
    const json=value=>new Response(JSON.stringify(value));
    const nativeFetch=async(url,options)=>{
      if(!url.startsWith('https://getbuild.ing/'))return fetch(url,options);
      assert.equal(options.headers.Authorization,'Bearer fixture-hosted-token');
      if(url.endsWith('/challenges'))return json({challenge_id:'challenge',nonce:'n'.repeat(32),issuer:'https://getbuild.ing',expires_at:Date.now()/1000+60});
      return json({issuer:'https://getbuild.ing',account_id:'account',device_id:'device',origin,endpoint_epoch:'epoch',family_id:'family',access_token:'fixture-device-token',expires_at:Math.floor(Date.now()/1000)+300,scope:'bridge:read bridge:write'});
    };
    const owner={id:1};let done;
    const received=new Promise(resolve=>done=resolve);
    const adapter=createBridgeHttp({fetch:nativeFetch,getAuth:()=>({accessToken:'fixture-hosted-token',generation:1}),isOwnerCurrent:o=>o===owner,send:(o,event)=>{
      assert.equal(o,owner);assert.equal(event.requestId,'00000000-0000-4000-8000-000000000001');assert.equal(typeof event.handle,'string');adapter.ack(owner,event.stream,event.sequence);
      if(event.event==='changes')done(event.data);
    },manifest:resourceManifest});
    try {
      const {handle}=await adapter.discover(owner,origin);
      const opened=await adapter.open(owner,handle,'1');assert.equal(opened.opening_generation,'generation');
      const {stream}=await adapter.subscribe(owner,handle,{kind:'application'},{requestId:'00000000-0000-4000-8000-000000000001'});assert.equal(typeof stream,'string');
      assert.deepEqual(await received,{method:'changes',params:{project_id:'project'}});
      assert.deepEqual(await adapter.request(owner,handle,'project_list',{}, {requestId:'request'}),{id:'request',ok:true,result:{projects:['project']},bridge_epoch:"epoch"});
      await adapter.close(owner,handle);
      await assert.rejects(adapter.open(owner,handle,'1'),{code:'invalid_handle'});
    } finally {adapter.clear();}
  `;
  const child = spawn(process.execPath, ["--input-type=module", "-e", source], {
    env: { ...process.env, NODE_EXTRA_CA_CERTS: cert },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stderr = "";
  child.stderr.on("data", (data) => (stderr += data));
  const [code] = await once(child, "close");
  assert.equal(code, 0, stderr);
  assert.equal(requests[0].headers.authorization, undefined);
  assert.deepEqual(JSON.parse(requests[0].body), {
    challenge_id: "challenge",
    nonce: "n".repeat(32),
  });
  for (const request of requests.slice(1))
    assert.equal(request.headers.authorization, "Bearer fixture-device-token");
  const resource = requests.find((request) => request.url === "/v1/projects");
  assert.equal(resource.method, "GET");
  assert.equal(resource.body, "");
  assert.equal(resource.headers["content-type"], undefined);
  assert.equal(resource.headers["x-build-session"], "session");
  assert.equal(resource.headers["x-build-subscription"], "subscription");
  assert.equal(resource.headers["x-build-request-id"], "request");
  assert.equal(requests.at(-1).method, "DELETE");
});
