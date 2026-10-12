import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { desktopReleaseAssets } from "../../scripts/desktop-release-assets.mjs";

const repository = fileURLToPath(new URL("../../", import.meta.url));
const workflow = readFileSync(join(repository, ".github/workflows/release-desktop.yml"), "utf8").replace(/\r\n/g, "\n");
const version = JSON.parse(readFileSync(join(repository, "desktop/package.json"))).version;
const expected = desktopReleaseAssets(version);
// Execute the Ubuntu publisher's literal shell blocks, not a reimplementation.
// Only checkout, download-artifact and external network tools are substituted.
const publisher = workflow.split("  publish:\n")[1];
const steps = publisher.split(/^      - /m).slice(1);
function property(step, key) {
  return step.match(new RegExp(`^\\s+(?:${key}): (.+)$`, "m"))?.[1];
}
function script(step) {
  const block = step.match(/^        run: \|\n((?:          .*\n|\n)*)/m)?.[1];
  return block?.replace(/^          /gm, "");
}

const networkAdapter = String.raw`#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const tool = path.basename(process.argv[1]);
const args = process.argv.slice(2);
const option = name => args[args.indexOf(name) + 1];
fs.appendFileSync(process.env.CALLS, JSON.stringify({tool, args, cwd: process.cwd()}) + '\n');
if (tool === 'cosign') {
  const checksums = fs.readFileSync(args.at(-1), 'utf8');
  if (args[0] === 'sign-blob') {
    fs.writeFileSync(option('--bundle'), JSON.stringify({checksums}));
  } else {
    assert.equal(args[0], 'verify-blob');
    assert.equal(JSON.parse(fs.readFileSync(option('--bundle'))).checksums, checksums);
    assert.equal(option('--certificate-identity-regexp'), process.env.COSIGN_IDENTITY_REGEXP);
    assert.equal(option('--certificate-oidc-issuer'), process.env.COSIGN_ISSUER);
    if (process.env.FAIL_VERIFY === '1') process.exit(1);
  }
  process.exit(0);
}
assert.equal(tool, 'gh');
assert.equal(args[0], 'release');
const state = JSON.parse(fs.readFileSync(process.env.STATE));
const tag = args[2];
if (args[1] === 'view') {
  if (!state[tag]) process.exit(1);
  if (args.includes('--json')) console.log(state[tag].pointer === null ? '0' : '1');
} else if (args[1] === 'create') {
  if (args.includes('--notes-file')) assert.match(fs.readFileSync(option('--notes-file'), 'utf8'), /Build Desktop/);
  state[tag] = {pointer: null};
  fs.writeFileSync(process.env.STATE, JSON.stringify(state));
} else if (args[1] === 'download') {
  if (process.env.FAIL_DOWNLOAD === '1') process.exit(1);
  fs.writeFileSync(path.join(option('--dir'), 'version.txt'), state[tag].pointer);
} else {
  assert.equal(args[1], 'upload');
  const files = args.slice(args.indexOf('--clobber') + 1);
  const destination = path.join(process.env.UPLOADS, tag);
  fs.mkdirSync(destination, {recursive: true});
  for (const file of files) fs.copyFileSync(file, path.join(destination, path.basename(file)));
}
`;

function fixture(t, options = {}) {
  const root = mkdtempSync(join(tmpdir(), "desktop publisher "));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const checkout = join(root, "checkout");
  const runnerTemp = join(root, "runner temp");
  const bin = join(root, "bin");
  for (const path of [checkout, runnerTemp, bin, join(checkout, "scripts")]) mkdirSync(path, { recursive: true });
  cpSync(join(repository, "assets"), join(checkout, "assets"), { recursive: true });
  cpSync(join(repository, "scripts/desktop-release-assets.mjs"), join(checkout, "scripts/desktop-release-assets.mjs"));
  for (const name of ["cosign", "gh"]) writeFileSync(join(bin, name), networkAdapter, { mode: 0o755 });
  const state = options.channel === undefined ? {} : {"desktop-latest": {pointer: options.channel}};
  if (options.releaseExists) state[`desktop-v${version}`] = {pointer: null};
  writeFileSync(join(root, "state.json"), JSON.stringify(state));
  writeFileSync(join(root, "calls.jsonl"), "");
  const context = {
    "runner.temp": runnerTemp,
    "needs.version.outputs.version": version,
    "needs.version.outputs.releases_repo": "fixture/releases",
    "github.ref_name": `desktop-v${version}`,
    "github.sha": "fixture-commit",
    "github.repository": "fixture/source",
    "secrets.RELEASES_TOKEN": "offline-fixture",
  };
  const env = {
    ...process.env, PATH: `${bin}:${process.env.PATH}`, GITHUB_WORKSPACE: checkout, RUNNER_TEMP: runnerTemp,
    CALLS: join(root, "calls.jsonl"), STATE: join(root, "state.json"), UPLOADS: join(root, "uploads"),
    FAIL_VERIFY: options.failVerify ? "1" : "0", FAIL_DOWNLOAD: options.failDownload ? "1" : "0",
  };
  for (const [, key, value] of workflow.split("jobs:\n")[0].matchAll(/^  (COSIGN_\w+): (.+)$/gm)) env[key] = value.replace(/^'|'$/g, "");
  return { root, checkout, runnerTemp, context, env };
}

function expand(value, context) {
  return value.replace(/\$\{\{\s*([^}]+?)\s*\}\}/g, (_, key) => {
    assert.ok(Object.hasOwn(context, key), `Unsupported workflow expression: ${key}`);
    return context[key];
  });
}

function runPublisher(f, contaminate = () => {}) {
  for (const [index, step] of steps.entries()) {
    const command = script(step);
    if (!command) {
      const action = step.match(/^uses: (.+)/)?.[1];
      if (action?.startsWith("actions/download-artifact@")) {
        assert.equal(property(step, "pattern"), "desktop-*");
        assert.equal(property(step, "merge-multiple"), "true");
        f.stage = resolve(f.checkout, expand(property(step, "path"), f.context));
        mkdirSync(f.stage, { recursive: true });
        for (const name of expected) writeFileSync(join(f.stage, name), `distributable bytes: ${name}\n`);
        contaminate(f.stage);
      } else {
        assert.match(action ?? "", /^(actions\/checkout|sigstore\/cosign-installer)@/);
      }
      continue;
    }
    const env = { ...f.env, GITHUB_OUTPUT: join(f.root, `output-${index}`) };
    const block = step.match(/^        env:\n((?:          .*\n)*)/m)?.[1] ?? "";
    for (const [, key, value] of block.matchAll(/^          (\w+): (.+)$/gm)) env[key] = expand(value, f.context);
    const cwd = resolve(f.checkout, expand(property(step, "working-directory") ?? ".", f.context));
    const result = spawnSync("bash", ["--noprofile", "--norc", "-eo", "pipefail", "-c", expand(command, f.context)], {
      cwd, env, encoding: "utf8", timeout: 5000,
    });
    if (result.status !== 0) return { ...result, step: step.split("\n")[0] };
    const id = step.match(/^id: (.+)/)?.[1] ?? property(step, "id");
    if (id && existsSync(env.GITHUB_OUTPUT)) {
      for (const line of readFileSync(env.GITHUB_OUTPUT, "utf8").trim().split("\n")) {
        const separator = line.indexOf("=");
        f.context[`steps.${id}.outputs.${line.slice(0, separator)}`] = line.slice(separator + 1);
      }
    }
  }
  return { status: 0 };
}
function calls(f) {
  return readFileSync(f.env.CALLS, "utf8").trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
}
function assertPublished(f, channelUploaded = true) {
  const release = join(f.env.UPLOADS, `desktop-v${version}`);
  assert.deepEqual(readdirSync(release).sort(), [...expected, "SHA256SUMS", "SHA256SUMS.sigstore.json"].sort());
  const checksums = readFileSync(join(release, "SHA256SUMS"), "utf8").trim().split("\n");
  assert.equal(checksums.length, 9);
  for (const line of checksums) {
    const [digest, name] = line.split("  ");
    assert.ok(expected.includes(name));
    assert.equal(digest, createHash("sha256").update(readFileSync(join(release, name))).digest("hex"));
  }
  const events = calls(f);
  const verify = events.findIndex(event => event.tool === "cosign" && event.args[0] === "verify-blob");
  const upload = events.findIndex(event => event.tool === "gh" && event.args[1] === "upload");
  assert.ok(verify >= 0 && upload > verify);
  assert.ok(f.stage.startsWith(`${f.runnerTemp}/`), "artifacts must be isolated from checkout in runner.temp");
  for (const event of events.filter(event => event.tool === "cosign" || event.args[1] === "upload")) assert.equal(event.cwd, f.stage);
  assert.ok(existsSync(join(f.checkout, "assets/brand/build-mark.svg")));
  assert.deepEqual(readdirSync(join(f.checkout, "assets")), ["brand"]);
  const pointer = join(f.env.UPLOADS, "desktop-latest/version.txt");
  assert.equal(existsSync(pointer), channelUploaded);
  if (channelUploaded) {
    assert.equal(readFileSync(pointer, "utf8"), `${version}\n`);
    assert.ok(events.findIndex(event => event.args[1] === "upload" && event.args[2] === "desktop-latest") > upload);
  }
}

// The workflow publisher runs on Ubuntu; native builds keep their separate gates.
const ubuntuPublisher = { skip: process.platform !== "linux" };
test("publisher ignores checked-out assets/brand through signed release and new channel upload", ubuntuPublisher, t => {
  const f = fixture(t);
  const result = runPublisher(f);
  assert.equal(result.status, 0, `${result.step}\n${result.stderr}`);
  assertPublished(f);
});
for (const [label, channel, channelUploaded] of [["empty pointer", null, true], ["older pointer", "0.1.0", true], ["equal pointer", version, true], ["newer pointer", "99.0.0", false]]) {
  test(`publisher preserves existing release/channel handling: ${label}`, ubuntuPublisher, t => {
    const f = fixture(t, { channel, releaseExists: true });
    const result = runPublisher(f);
    assert.equal(result.status, 0, `${result.step}\n${result.stderr}`);
    assertPublished(f, channelUploaded);
    assert.ok(!calls(f).some(event => event.args[1] === "create"));
  });
}
for (const [label, options] of [["invalid pointer", {channel: "invalid"}], ["channel download failure", {channel: "0.1.0", failDownload: true}], ["signature verification failure", {failVerify: true}]]) {
  test(`publisher stops safely on ${label}`, ubuntuPublisher, t => {
    const f = fixture(t, options);
    assert.notEqual(runPublisher(f).status, 0);
    assert.ok(!existsSync(join(f.env.UPLOADS, "desktop-latest/version.txt")));
    if (options.failVerify) assert.ok(!existsSync(f.env.UPLOADS));
  });
}
test("publisher still rejects unexpected files inside artifact staging before signing/upload", ubuntuPublisher, t => {
  const f = fixture(t);
  const result = runPublisher(f, stage => writeFileSync(join(stage, "unexpected.txt"), "refuse me"));
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Unexpected desktop release asset: unexpected.txt/);
  assert.deepEqual(calls(f), []);
  assert.ok(!existsSync(join(f.stage, "SHA256SUMS")));
});
