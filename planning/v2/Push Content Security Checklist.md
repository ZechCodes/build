# Push content security checklist

**Status:** verified (10/10 controls)

**Scope:** sealed push content, #200. A browser push says what happened (a
task's title and its news, an agent's name and the first line of what it
said) while the api, the push service and every log see only ciphertext. The
#191 content-free pushes this builds on, and the E2EE session itself, are not
rescored here.

## The scheme

Everything below is exact: the bridge (`bridge/src/notify/seal.rs`), the
service worker (`spa/public/sw.js`) and the fixture generator
(`fixtures/push/generate.py`) implement it independently, and
`fixtures/push/sealed-v1.json` holds them to it.

**Encoding.** `b64u(x)` is unpadded base64url. Strings are UTF-8.

**Subscription id.** `sid = b64u(SHA-256(push endpoint))`, 43 characters. The
SPA and the service worker compute it from their own `PushSubscription`, and
the api computes it from the endpoint it already stores, so no table changes.
The bridge learns only the hash, never the endpoint.

**Notification key.** One ECDH P-256 key pair per subscription, generated in
the browser by WebCrypto with `extractable: false`.

- The private `CryptoKey` lives in IndexedDB (database `build-push`, store
  `keys`, keyed by sid), where the service worker reads it.
- The public key, `b64u(uncompressed point)` (65 bytes), goes to the bridge only
  over the E2EE session (`push.registerKey`).
- A new subscription is a new sid and a new key, and the old key is deleted.

**Sealed blob, version 1.**

```
blob      = b64u(0x01 ‖ epk ‖ nonce ‖ ciphertext‖tag)
epk       = the ephemeral P-256 public key, uncompressed (65 bytes), fresh per blob
nonce     = 12 random bytes
shared    = ECDH(ephemeral private, recipient public), the 32-byte x coordinate
key       = HKDF-SHA256(salt = empty, ikm = shared,
                        info = "build-push-v1" ‖ epk ‖ recipient public, L = 32)
aad       = "build-push-v1" ‖ 0x00 ‖ sid ‖ 0x00 ‖ kind ‖ 0x00 ‖ entity id
ciphertext‖tag = AES-256-GCM(key, nonce, plaintext, aad), 16-byte tag
plaintext = JSON {"v": 1, "title", "body", "url", "iat"}
```

- The AAD binds the blob to its subscription and to the cleartext `kind` and
  `task_id` the api wraps it in. A blob moved to another subscription, or
  pasted beside another entity, fails the tag.
- `title` is at most 64 characters and `body` at most 160. Each is the first
  non-empty line of its source with whitespace collapsed, cut on a character
  boundary and ended with `…` when cut.
- `url` is a same-origin deep link starting `/app/#/`.
- `iat` is the bridge's unix seconds when it sealed.
- The plaintext is at most 1 KiB and the blob at most 2048 characters, well
  under the ~4 KB a Web Push message carries after its own encryption.

**Authenticity, stated exactly.** This is ECIES with an ephemeral sender key:
it is **not sender-authenticated**. Anyone holding a subscription's
notification public key can seal a blob that decrypts. The only thing that
stops the api, the push service or anyone else from forging content is that
the public key stays secret: it is generated in the browser and travels only
over the E2EE session to the bridge. The api never receives it. As defence in
depth, even a forged blob cannot navigate outside the app: the service worker
refuses any sealed `url` that does not start with `/app/#/`, and so does the
app's `build.push.open` listener.

**Freshness and replay.** The service worker accepts a blob only if
`now − (PUSH_TTL_SECONDS + CLOCK_SKEW_SECONDS) ≤ iat ≤ now + CLOCK_SKEW_SECONDS`.
`PUSH_TTL_SECONDS` is the TTL the api sets on every push (`web_push.py`), and
`CLOCK_SKEW_SECONDS` is 300. It also drops a blob whose nonce it has already
seen: nonces seen inside that window are kept in IndexedDB (store `seen`),
pruned by age and capped at 256. There is no ordering check, because several
bridges with different clocks share one sid and the push service does not
order delivery. A blob that fails either check shows the generic copy.

## The wire

- **Bridge RPC, wire 2.1.0.** `push.registerKey {subscription_id, public_key}`
  upserts a key; the point is validated at registration. `push.revokeKey
  {subscription_id}` deletes it. Both return `{}` and are announced by name in
  `capabilities`. The SPA calls them only on a bridge that announces them.
- **Bridge store, schema 11.** `push_keys(subscription_id PRIMARY KEY,
  public_key, registered_at)`, at most 32 rows, the oldest evicted.
- **Notify request.** Adds `sealed: [{subscription_id, blob}]`, one entry per
  registered key the bridge could seal to. With `sealed` present the signed
  challenge is `notify.{device}.{entity}.{kind}.{timestamp}.{digest}`, where
  `digest` is the lowercase hex SHA-256 of the concatenated
  `"{subscription_id}:{blob}\n"` lines in request order. Without it the #191
  challenge is unchanged.
- **Notify reply.** Adds `unknown_subscriptions`: the sealed sids that match
  none of the owner's subscriptions after delivery (gone ones included). The
  bridge deletes those keys.
- **Push payload.** A subscription with a blob gets `{task_id, kind, url,
  sealed}`; one without gets the #191 `{task_id, kind, url}`.

## Failure modes

| Case | Behaviour |
|---|---|
| Old bridge, no registered key, key registered with another bridge only | No blob for that subscription; generic copy |
| Old service worker | Ignores `sealed`; generic copy |
| Rotated or deleted key, bad tag, wrong AAD, stale or future `iat`, seen nonce, malformed plaintext | Generic copy |
| Sealing fails for one key | That subscription gets no blob; the notify still goes out |
| Content cannot be built | No blobs; the notify still goes out |
| Subscription gone | The api reports its sid unknown; the bridge prunes the key |
| Notification on a lock screen | Shows the content. Zech's call (23:25Z Sep 27): no setting |

## Controls

| # | Control | Status | Required evidence |
|---|---|---|---|
| 1 | The api, the push service and the logs see only sids and ciphertext: no title, body or url text leaves the bridge outside a sealed blob, and no bridge log line carries it. | [x] | `sealed_entries_carry_the_words_and_the_request_carries_none` (no title, body or url anywhere in the POST), `no_log_line_on_the_push_path_formats_content_or_keys` (a source scan of every `eprintln!`/`log::` on the push path; a probe line formatting `{title}` failed it), `debug_never_prints_the_words`, `a_keys_debug_print_hides_the_key`, `errors_name_the_failure_and_nothing_else`. |
| 2 | The api forwards each blob byte-identical, never decodes, parses, stores or logs it, and validates only its shape. | [x] | `test_a_sealed_notify_forwards_the_blob_byte_identical_to_its_subscription`, `test_a_blob_that_is_not_ciphertext_is_forwarded_unchanged`, `test_a_failed_sealed_delivery_logs_neither_blob_nor_payload`, `test_a_delivery_failure_never_logs_the_blob_or_payload` (transport, push-service error, gone), `test_the_real_sender_never_logs_the_blob_or_payload`, `test_a_badly_shaped_sealed_is_refused` (20 cases). The delivery-failure warning now logs the sid and exception type only. |
| 3 | The three implementations agree on the fixture: Rust reproduces the blob byte-for-byte and opens it, Chromium WebCrypto opens it. | [x] | `the_seal_reproduces_the_fixture_blob_byte_for_byte`, `the_fixture_blob_opens_to_the_fixture_plaintext`; Chromium 152 `opens the fixture with Chromium's WebCrypto and shows its title and body` (`spa/test/browser/pushSealedChromium.test.js`); the SPA's test sealer also reproduces the blob; `test_notify_challenge_v2_equals_the_fixture` and `the_v2_challenge_matches_the_fixture` hold the signed challenge. |
| 4 | The AAD binds sid, kind and entity id: changing any one fails decryption. | [x] | `changing_any_one_of_sid_kind_or_entity_fails_to_open` (Rust); `pushSealed.test.js` tampered sid, kind and entity each show the generic copy, and the sid is computed from the worker's own subscription, never the payload; Chromium tampered-entity case. |
| 5 | The notification private key is non-extractable, lives only in the browser, and its public key travels only over E2EE. | [x] | `pushKeys.js` generates with `extractable: false`; the Chromium end-to-end case logs `private key extractable=false`; `pushKeySync.test.js` registers only over `push.registerKey` on bridges that announce it; no api route accepts a notification key. Authenticity rests on this and is stated as such above: the sealing is not sender-authenticated. |
| 6 | Freshness and nonce replay: out-of-window and repeated blobs show the generic copy; blobs from bridges with skewed clocks inside the window, in any order, show content. | [x] | `pushSealed.test.js`: window edges at +300/+301/−300/−301 s, stale and future `iat`, replay across a fresh worker, skewed bridges out of order, the 256 cap and pruning by age, `PUSH_TTL_SECONDS` equal to `web_push.py`'s; Chromium end to end: the same blob delivered twice shows content then the generic copy. |
| 7 | Deep links stay inside the app: the service worker and the app's message listener refuse anything outside `/app/#/`, and the listener accepts messages only from this origin's active service worker. | [x] | `pushSealed.test.js` sealed url outside `/app/#/` refused; `pushOpen.test.js` rejects six wrong sources and eight off-app urls and accepts the active worker and the controller; `sw.test.js` postMessage, navigate fallback and cold-start `openWindow`; `shellLinkedAgent.test.js` and `agentRailDom.test.js` open the linked chat at its latest message. |
| 8 | Every failure falls back to the #191 generic notification, and content building or sealing never fails or delays the notify itself. | [x] | `a_bad_stored_key_falls_back_to_generic_and_the_notify_still_posts`, `a_bad_key_beside_a_good_one_costs_only_its_own_entry`, `without_keys_or_content_the_request_is_the_191_shape`; sealing runs in the spawned task on the blocking pool within a 1 s budget, never under the app lock (`notify/delivery.rs`); `pushSealed.test.js` generic copy for no key, no subscription, malformed blob or plaintext, and old payloads; `test_a_v1_notify_without_sealed_still_pushes_the_191_payload`. |
| 9 | Keys over time: rotate on re-subscribe, revoke on disable, prune what the api reports unknown, bounded storage; schema 10 → 11 keeps existing data. | [x] | `pushKeySync.test.js` rotation, enable and disable/revoke; `the_keys_the_api_reports_unknown_are_forgotten`, `a_refused_notify_forgets_no_key` (only keys actually sealed to are pruned, so the api cannot delete others); `registering_past_the_cap_evicts_the_oldest`; `a_v10_store_opens_at_schema_11_with_push_keys_and_its_data_intact`; `test_sealed_sids_matching_no_live_subscription_are_reported_unknown`. |
| 10 | Full gates and scans pass on the completed tree. | [x] | Recorded below. |

## Final gate evidence

All under `nice -n 10`, judged by exit code, on the completed tree.

- Bridge: `cargo fmt --check` 0, `cargo clippy --all-targets -- -D warnings` 0,
  `cargo test --no-fail-fast` 0 (3,594 passed, 0 failed, 8 ignored). The
  vendored webrtc gate was not run: nothing under `bridge/vendor` changed.
- SPA: `npm run lint` 0, `npm test` 0 (485 files, 7,934 tests, Chromium
  browser tests included), `npm run build` 0.
- skriftapp: `ruff check buildapp` 0, `pytest buildapp -q` 0 (694 passed, 1
  skipped).
- `semgrep --config auto --error` on the 58 changed js/mjs/css/py/rs files: 0
  findings. `gitleaks git --log-opts=main..HEAD`: no leaks.
  `git diff --check main..HEAD`: clean. No shell file changed. No complexity
  ratchet entry was added.
- Negative controls, each restored after: logging the exception text, dropping
  the sealed digest, altering the blob (api); changing the AAD separator,
  disabling the prune filter and the sid dedupe, a probe log line formatting
  `{title}` (bridge); ignoring the AAD, removing the nonce cap, removing the
  listener's source or url check, removing register/revoke, reverting the
  land-on-latest change (SPA). Each made its tests fail.

**Final score: 100/100.** The notification's content shows on a lock screen by
Zech's decision (no setting); that is a product choice, not an open control.
