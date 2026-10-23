// Browser origin isolation for the dev deployment. Only ephemeral loopback
// servers and a fresh browser profile; no bridge or external hosts are used.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { test } from "node:test";
import { chromium } from "playwright";

async function originServer() {
  const server = createServer((request, response) => {
    if (request.url === "/app/sw.js") {
      response.writeHead(200, { "content-type": "text/javascript", "cache-control": "no-cache" });
      response.end("self.addEventListener('install', () => self.skipWaiting()); self.addEventListener('activate', e => e.waitUntil(self.clients.claim()));");
    } else {
      response.writeHead(200, { "content-type": "text/html" });
      response.end("<!doctype html><title>Disposable origin</title>");
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return { server, url: `http://127.0.0.1:${server.address().port}` };
}

async function store(page, marker) {
  return page.evaluate(async value => {
    localStorage.setItem("build", value);
    for (const name of ["build-cache", "build-ui", "build-push"]) {
      const db = await new Promise((resolve, reject) => {
        const request = indexedDB.open(name, 1);
        request.onupgradeneeded = () => request.result.createObjectStore("records");
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
      });
      await new Promise((resolve, reject) => {
        const tx = db.transaction("records", "readwrite");
        tx.objectStore("records").put(value, "marker");
        tx.oncomplete = resolve;
        tx.onerror = () => reject(tx.error);
      });
      db.close();
    }
    const cache = await caches.open("build-cache");
    await cache.put("/app/marker", new Response(value));
    await navigator.serviceWorker.register("/app/sw.js", { scope: "/app/" });
    await navigator.serviceWorker.ready;
    globalThis.messages = [];
    globalThis.channel = new BroadcastChannel("build");
    channel.onmessage = event => messages.push(event.data);
  }, marker);
}

async function read(page) {
  return page.evaluate(async () => {
    const values = [];
    for (const name of ["build-cache", "build-ui", "build-push"]) {
      const db = await new Promise(resolve => {
        const request = indexedDB.open(name);
        request.onsuccess = () => resolve(request.result);
      });
      values.push(await new Promise(resolve => {
        db.transaction("records").objectStore("records").get("marker").onsuccess = event => resolve(event.target.result);
      }));
      db.close();
    }
    return {
      local: localStorage.getItem("build"), databases: values,
      cache: await (await (await caches.open("build-cache")).match("/app/marker")).text(),
      workers: (await navigator.serviceWorker.getRegistrations()).map(registration => registration.scope),
      messages,
    };
  });
}

test("same storage, worker, cache and channel names stay separate across two origins", async () => {
  const origins = [];
  let browser;
  try {
    origins.push(await originServer(), await originServer());
    browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || "/usr/bin/chromium", args: ["--no-sandbox"] });
    const context = await browser.newContext();
    const pages = await Promise.all(origins.map(() => context.newPage()));
    for (let index = 0; index < pages.length; index++) {
      await pages[index].goto(`${origins[index].url}/app/`);
      await store(pages[index], String(index));
    }
    const sibling = await context.newPage();
    await sibling.goto(`${origins[0].url}/app/`);
    await sibling.evaluate(() => {
      globalThis.channel = new BroadcastChannel("build");
      channel.postMessage("prod-only");
    });
    await pages[0].waitForFunction(() => messages.includes("prod-only"));
    for (let index = 0; index < pages.length; index++) {
      const result = await read(pages[index]);
      assert.equal(result.local, String(index));
      assert.deepEqual(result.databases, Array(3).fill(String(index)));
      assert.equal(result.cache, String(index));
      assert.deepEqual(result.workers, [`${origins[index].url}/app/`]);
      assert.deepEqual(result.messages, index === 0 ? ["prod-only"] : []);
    }
  } finally {
    await browser?.close();
    for (const { server } of origins) {
      server.closeAllConnections();
      await new Promise(resolve => server.close(resolve));
    }
  }
});
