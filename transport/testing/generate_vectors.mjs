// Complete Python's vector JSON with deterministic envelopes from a chosen JS source.
import { readFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [source, input] = process.argv.slice(2);
if (!source || !input) throw new Error("usage: node generate_vectors.mjs <JS package> <Python vector JSON>");
const binding = await import(pathToFileURL(path.resolve(source, "src/index.js")));
const v = JSON.parse(readFileSync(input, "utf8"));
for (const frame of v.frames) {
  frame.envelopes.js = await binding.encryptFrame({ sessionKeyB64: v.session_key,
    outerFields: frame.outer, frameFields: frame.fields, nonceB64: frame.js_nonce });
}
for (const entry of v.envelope_cases) {
  try {
    await binding.decryptEnvelope({ sessionKeyB64: v.session_key, envelope: entry.envelope });
    entry.accepts.js = true;
  } catch {
    entry.accepts.js = false;
  }
}
console.log(JSON.stringify({ envelopes: v.frames.map(frame => frame.envelopes.js),
  accepts: v.envelope_cases.map(entry => entry.accepts.js) }, null, 2));
