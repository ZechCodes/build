import { whenGreeted } from "./deviceContexts.js";
import { bridgeCapabilities } from "./changeEvents.js";

const writeCapabilities = { "fs.uploadBegin": "uploads", "fs.createDirectory": "createDirectory" };

/** Rendered support is cached; each new write uses the current greeting. */
export function fileUploadRpc(context, callRpc = context.rpc) {
  return async (method, params, ...options) => {
    const request = await whenGreeted(context, () => {
      const capability = writeCapabilities[method];
      if (capability && !bridgeCapabilities(context.deviceId).fs?.[capability]) {
        throw new Error("Update the bridge to upload files or create folders.");
      }
      return callRpc(method, params, ...options);
    });
    if (!request) throw new Error("This machine is unavailable.");
    return request.sent;
  };
}
