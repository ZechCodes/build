# Fixed transport compatibility vectors

`v1.json` is public test material from Git blobs at upstream
`4bce3aacebd66a9674aaa96c6ec0d97ab4bcd793`; `SHA256SUMS` freezes its bytes.
The Python and JS runtime blob digests remain recorded in `../UPSTREAM.md`.
The fixture covers Ed25519 signing/fingerprint, X25519 sealed bootstrap,
session_accept, data/close, Unicode/nested payloads, numeric JSON differences,
wrong key/signature, ciphertext tampering, session mismatch and parser inputs.
The seed, transport private key and session key are deliberately public sequences
of bytes. Never use any fixture key/nonce for a real session.

The bootstrap is a captured randomized sealed box. Consumers must decrypt it
and compare the recovered key; fresh wraps are never compared for byte equality.
Accept and frames use fixed nonces/message ids/timestamps. Every distinct
plaintext has a distinct nonce, including Python and JS producer variants.
Deterministic regeneration is compared within the same language. Cross-language
consumption compares the recovered fields/payload; JSON serialization is not
claimed universally identical. Rust consumes common static vectors and talks to
Python through its own required interop test. There is no direct JS/Rust runtime
exchange test.

Generate once from a read-only Git-blob export of the pin, with frozen dependency
resolution. The source checkout itself must never be modified. From Build:

```sh
nice -n 10 npm ci --prefix <export>/js --ignore-scripts
PYTHONPATH=<export>/python/src UV_CONCURRENT_BUILDS=1 nice -n 10 uv run --frozen \
  --project transport/python python transport/testing/generate_vectors.py > python.json
nice -n 10 node transport/testing/generate_vectors.mjs <export>/js python.json > js.json
```

The JS command emits envelopes and acceptance flags only. Merge those into
Python's JSON with Python, preserving its `1.0` numeric lexeme. The stored file
uses sorted keys, UTF-8 and two-space indentation, with one final newline.
Regeneration experiments can use `BST_FIXTURES=<json>`; the pinned digest is
still checked. `BUILD_SECURE_TRANSPORT_JS=<package>` and Python `PYTHONPATH`
allow deliberate old-source probes. Normal tests require only this Build tree.

Observed pre-move parser baseline (runtime behavior is preserved):

| Envelope input | Python | JS | Rust |
| --- | --- | --- | --- |
| missing version | accepts as 1 | accepts as 1 | typed field required |
| null version | rejects | accepts as 1 | rejects |
| boolean `true` version | accepts (`True == 1`) | rejects | rejects |
| JSON `1.0` version | accepts | accepts | rejects typed integer |
| padded ciphertext Base64 | accepts | rejects | accepts (trailing padding stripped) |
| unknown outer field | ignores | ignores | ignores |
| version 2, bad Base64, tamper, session mismatch | rejects | rejects | rejects |

Python preserves JSON float `1.0`, while JS serializes it as `1`. Python sorts
object keys lexically; JS enumerates integer-like keys numerically even after
sorting. Both variants decrypt to equivalent payload values. These fixtures
record differences; they do not define a stricter new protocol. Route metadata
is not authenticated and these bindings do not provide replay filtering.

Scoped checks (run under nice 10; coordinate Rust with the gate scheduler):

```sh
nice -n 10 uv run --frozen --project transport/python python -m unittest discover -s transport/python/tests -v
nice -n 10 uv run --frozen --project transport/python python -m unittest discover -s transport/compatibility -v
nice -n 10 npm test --prefix transport/js
nice -n 10 python -m unittest discover -s transport/tests -v
# from bridge/, capped CARGO_BUILD_JOBS and RUST_TEST_THREADS
nice -n 10 cargo test --locked --lib transport::tests -- --test-threads=1
nice -n 10 cargo test --locked --test interop_python -- --test-threads=1 --nocapture
nice -n 10 cargo test --locked --test transport_vectors -- --test-threads=1
```

The original Python suite remains five cases, and the original JS suite remains
four cases. Added vector suites contain four Python, four JS and three Rust
tests; JS's default `npm test` includes both its baseline and vector suites.
The command-level launcher suite adds four stdlib Python cases. For an
unconfigured uv shim, set `BST_UV` to an existing uv binary for JS/Rust and invoke
that binary explicitly for Python checks; do not change host installation policy.
