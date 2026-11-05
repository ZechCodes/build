// Build-owned additions wrap the byte-preserved imported transport API.
export * from './index.js';
import { sealSecretUploadWithSodium } from './secretsUpload.js';

let sodiumReadyPromise;

async function uploadSodium() {
  if (!sodiumReadyPromise) {
    sodiumReadyPromise = (async () => {
      let sodiumModule;
      try {
        sodiumModule = await import('libsodium-wrappers');
      } catch (error) {
        if (!(typeof process !== 'undefined' && process.versions?.node)) throw error;
        const { createRequire } = await import('node:module');
        const require = createRequire(import.meta.url);
        sodiumModule = require('libsodium-wrappers');
      }
      const sodium = sodiumModule.default ?? sodiumModule;
      await sodium.ready;
      return sodium;
    })();
  }
  return sodiumReadyPromise;
}

// The caller supplies the device key from authenticated discovery. Sealed
// requests are ephemeral and never belong in a persisted write-intent queue.
export async function sealSecretUpload(options) {
  return sealSecretUploadWithSodium(await uploadSodium(), options);
}
