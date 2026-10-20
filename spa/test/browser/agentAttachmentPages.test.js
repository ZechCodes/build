import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, openLayoutSession } from "./layoutHarness.mjs";
import { mountPagedAgentAttachments } from "./agentAttachmentPagesSeed.mjs";

const PAGE_BYTES = 1024 * 1024;
const LARGE_BYTES = 5 * PAGE_BYTES + 123123;
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
let session;
beforeAll(async () => { session = await openLayoutSession(); });
afterAll(async () => { await session?.close(); });

function fileReplies(bytes, name, mime) {
  const path = `/store/${name}`;
  const reply = (offset, length) => ({ ok: true, result: {
    path, mime, size: bytes.length, offset, content_b64: bytes.subarray(offset, offset + length).toString("base64"),
  } });
  return {
    path, name, mime, size: bytes.length, sha256: sha256(bytes),
    default_reply: reply(0, 5 * PAGE_BYTES),
    ranged_replies: Array.from({ length: Math.ceil(bytes.length / PAGE_BYTES) }, (_, index) => reply(index * PAGE_BYTES, PAGE_BYTES)),
  };
}

async function syntheticFiles() {
  const image = await readFile(new URL("../../../design/tasks-dashboard/tasks111-desktop-dark.png", import.meta.url));
  const video = await readFile(new URL("../../../design/landing-review/mobile-animatic.webm", import.meta.url));
  // Valid media with trailing padding lets tiny tracked sources exercise files
  // larger than the wire cap, without checking large test binaries into git.
  const padded = (bytes) => Buffer.concat([bytes, Buffer.alloc(LARGE_BYTES - bytes.length)]);
  return [
    fileReplies(padded(image), "large-preview.png", "image/png"),
    fileReplies(padded(video), "large-recording.webm", "video/webm"),
    fileReplies(Buffer.alloc(LARGE_BYTES, 0x61), "large-verification.log", "text/plain"),
  ];
}

async function verifyDownload(page, file) {
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    page.getByTitle(`Download ${file.name}`).first().click(),
  ]);
  expect(download.suggestedFilename()).toBe(file.name);
  const bytes = await readFile(await download.path());
  expect(bytes.length).toBe(file.size);
  expect(sha256(bytes)).toBe(file.sha256);
}

async function heldMedia(page, path) {
  return page.evaluate(async (path) => {
    const blob = window.__pagedAttachmentState.attachment(path)?.body?.blob;
    if (!blob) return null;
    const digest = await crypto.subtle.digest("SHA-256", await blob.arrayBuffer());
    return { size: blob.size, mime: blob.type, sha256: [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("") };
  }, path);
}

it.each([true, false])("reconstructs full image, video and download bytes above 5 MiB with capability=%s", async (canPage) => {
  const files = await syntheticFiles();
  const payload = { entity_id: "large-evidence", agent_id: "large-agent", files, thread: { items: [{
    type: "message", data: { role: "agent", sequence: 1, body: "Large screenshot, recording and verification log.",
      attachments: files.map(({ path, name, mime, size }) => ({ path, name, mime, size })) },
  }] } };
  await session.withPage(async ({ page, basePath }) => {
    await mountPagedAgentAttachments(page, basePath, payload, canPage);
    await page.waitForFunction(() => {
      const image = document.querySelector(".thread-attachment-image");
      return image?.complete && image.naturalWidth > 0;
    });
    await page.getByRole("button", { name: "Open large-preview.png" }).click();
    await page.waitForFunction(() => document.querySelector(".thread-lightbox img")?.naturalWidth > 0);
    expect(await heldMedia(page, files[0].path)).toEqual({ size: files[0].size, mime: files[0].mime, sha256: files[0].sha256 });
    await page.keyboard.press("Escape");
    await page.locator(".thread-lightbox").waitFor({ state: "detached" });
    await page.getByRole("button", { name: "Play large-recording.webm" }).click();
    await page.waitForFunction(() => document.querySelector(".thread-lightbox video")?.readyState >= 2);
    expect(await heldMedia(page, files[1].path)).toEqual({ size: files[1].size, mime: files[1].mime, sha256: files[1].sha256 });
    // Let the lightbox reveal finish before optional review screenshots.
    await page.waitForTimeout(1000);
    await captureLayout(page, `task696-agent-attachments-large-video-${canPage}.png`);
    await page.keyboard.press("Escape");
    await page.locator(".thread-lightbox").waitFor({ state: "detached" });
    await verifyDownload(page, files[2]);
    const calls = await page.evaluate(() => window.__attachmentReadCalls);
    for (const file of files) {
      const reads = calls.filter(({ params }) => params.path === file.path);
      expect(reads.map(({ params }) => params.offset)).toEqual(canPage ? [0, PAGE_BYTES, 2 * PAGE_BYTES, 3 * PAGE_BYTES, 4 * PAGE_BYTES, 5 * PAGE_BYTES] : [undefined, 5 * PAGE_BYTES]);
      expect(reads.every(({ options }) => options.priority === "background")).toBe(true);
    }
  });
}, 60_000);

it("downloads every byte posted by real Rust MCP through the capped and ranged RPC reader", async () => {
  const binary = process.env.AGENT_ATTACHMENT_PROBE_BIN;
  const output = execFileSync(binary || "cargo", binary
    ? ["agent_attachment_chat_wire_probe", "--nocapture"]
    : ["test", "--lib", "agent_attachment_chat_wire_probe", "--", "--nocapture"], {
    cwd: resolve(process.cwd(), "../bridge"), encoding: "utf8", timeout: 900_000, maxBuffer: 40 * PAGE_BYTES,
  });
  const encoded = output.match(/AGENT_ATTACHMENT_WIRE=(\{[^\r\n]+\})/)?.[1];
  expect(encoded, output.slice(-2000)).toBeTruthy();
  const probe = JSON.parse(encoded);
  const { path, name, mime, size, sha256, default_reply, ranged_replies } = probe;
  const file = { path, name, mime, size, sha256, default_reply, ranged_replies };
  expect(file.size).toBeGreaterThan(5 * PAGE_BYTES);
  expect(Buffer.from(file.default_reply.result.content_b64, "base64").length).toBe(5 * PAGE_BYTES);
  await session.withPage(async ({ page, basePath }) => {
    await mountPagedAgentAttachments(page, basePath, {
      entity_id: probe.entity_id, agent_id: probe.agent_id, thread: probe.thread, files: [file],
    }, false);
    await verifyDownload(page, file);
    const calls = await page.evaluate(() => window.__attachmentReadCalls);
    expect(calls.map(({ params }) => params.offset)).toEqual([undefined, 5 * PAGE_BYTES]);
    await captureLayout(page, "task696-agent-attachments-rust-wire-download.png");
  });
}, 930_000);
