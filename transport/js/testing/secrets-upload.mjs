// Synthetic JS→Rust interoperability fixture. Emits ciphertext only.
import { sealSecretUpload } from '../src/index.js';
let input = '';
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
process.stdout.write(await sealSecretUpload(request));
