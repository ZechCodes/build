import { bridgeCapabilities } from "./changeEvents.js";
import { bytesOfBase64 } from "./bodyPages.js";

const PAGE_BYTES = 1048576;
const incomplete = () => new Error("The file changed or could not be read completely. Try downloading again.");

function spanContinues(span, offset, previous) {
  if (!Number.isSafeInteger(span.total) || span.total < span.end || span.end < offset) throw incomplete();
  if (span.end === offset && span.end !== span.total) throw incomplete();
  if (previous && (span.version !== previous.version || span.total !== previous.total)) throw incomplete();
}

function checkedPage(file, offset, previous) {
  const span = file.range;
  const bytes = bytesOfBase64(file.content_b64);
  if (file.truncated || !span || span.offset !== offset || span.end !== offset + bytes.length) throw incomplete();
  spanContinues(span, offset, previous);
  return { bytes, span };
}

async function readWholeFile(callRpc, params, deviceId) {
  const first = await callRpc("fs.read", params);
  if (!first.truncated) return new Blob([bytesOfBase64(first.content_b64)], { type: first.mime });
  if (!bridgeCapabilities(deviceId)?.bodies?.pages) throw new Error("Update the bridge to download this entire file.");
  const pieces = [];
  let offset = 0;
  let previous = null;
  do {
    const file = await callRpc("fs.read", { ...params, range: { offset, bytes: PAGE_BYTES } });
    const { bytes, span } = checkedPage(file, offset, previous);
    pieces.push(bytes);
    offset = span.end;
    previous = span;
  } while (offset < previous.total);
  return new Blob(pieces, { type: first.mime });
}

/** Save the complete scoped file, never a preview's truncated bytes. Reads
 * stay outside the viewer so a download cannot disturb its selection/cache. */
export async function downloadFile({ callRpc, scope = {}, path, deviceId }) {
  const blob = await readWholeFile(callRpc, { ...scope, path }, deviceId);
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = path.split("/").at(-1) || "download";
  document.body.append(link);
  try {
    link.click();
  } finally {
    link.remove();
    // Give mobile browsers time to hand the object URL to their download UI.
    setTimeout(() => URL.revokeObjectURL(url), 60000);
  }
}
