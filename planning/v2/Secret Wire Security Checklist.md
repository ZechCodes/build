# Secret wire security checklist

Scope: #1003 sealed upload, staging reconciliation, atomic send, metadata-only
public routes, invalidations and delivered message blocks. CLI execution and
composer UX are qualified by their own slices. Agents intentionally retrieve
raw values through the CLI; that output can enter their tool transcripts.

**Scoped score: 100/100.** Each row is 10 points. The final integrated run
passed all 206 targeted secret tests with no failures or ignored tests. Eighteen
assertions failed under reversible negative controls, then passed after byte-exact
source restoration. Public contracts, SQL publication ownership, all five harness
goldens, 336 targeted SPA tests and 19 HTTP coverage tests are verified.

| # | Control | Targeted evidence |
|---|---|---|
| 1 | Only authenticated device-user calls create/bind; internal, agent and tool posts cannot bind. HTTP and WebRTC share admission. | `app::secret_wire`, `secret_posts`, secret SQL read authority tests. |
| 2 | Upload uses existing sealed boxes, explicit domain/version and fixed 32 KiB padding; all context including grant is compared; requests cap at 64 KiB. | `secrets::upload`, JS sealing interoperability and tamper tests. |
| 3 | Plaintext/ciphertext envelopes never enter history, generic write persistence, cache or push. Public DTO is allowlisted. | staging SQL fault tests, API fixtures and metadata allowlist assertions. |
| 4 | Same request/exact sealed bytes reconciles after timeout/restart; changed content conflicts; upload/decrypt shares four-job bound. | `secrets::lifecycle::staging`, request-ID lookup tests. |
| 5 | Blob publication precedes metadata commit; failed SQL leaves inaccessible encrypted orphan; startup removes it. | staging persistence-boundary injection, existing vault crash tests. |
| 6 | Send commits complete set, message, descriptor and receipt atomically with FULL durability. Replay preserves original acceptance and TTL. | `secret_atomic`, `post_secret_claim`, failed binding rollback. |
| 7 | Existing/final provisional recipient is fenced by canonical project, owner, agent, conversation and generation. Immutable AAD stays intact. | `secret_post`, generation-race and provisional release tests. |
| 8 | Metadata and audit reads are bounded scoped snapshots; cursor scope changes are refused; tombstones and TTL remain authoritative. | `store::secrets::metadata`, malformed query/authorization tests. |
| 9 | Invalidation contains only IDs/revision/scope; project routing survives originating conversation retirement; lost/reordered hints reconcile without uploads. | `changes::secrets_tests`, SPA `secretsWire` tests. |
| 10 | Delivery exact revision block includes scope and no values, wrapped/pinned for all five harnesses; features remain unadvertised. | `agent_message::secret_tests`, API contract + withheld capability tests. |

Run only targeted gates under `nice -n 10`, with scratch CARGO_TARGET_DIR and
TMPDIR. The final task comment records exit codes, test counts, scanner results,
frozen head and merge overlap with the CLI slice. No installed bridge is run.
