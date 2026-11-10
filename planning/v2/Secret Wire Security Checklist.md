# Secret wire security checklist

Scope: #1003 sealed upload, staging reconciliation, atomic send, metadata-only
public routes, invalidations and delivered message blocks. CLI execution and
composer UX are qualified by their own slices. Agents intentionally retrieve
raw values through the CLI; that output can enter their tool transcripts.

**Scoped score: 100/100.** Each row is 10 points. The wire-3.36 rebase passed
270 targeted secret tests, 69 API contracts, three HTTP manifest tests and 52
subscription tests with no failures or ignored tests. SQL publication ownership,
all five harness goldens, 370 targeted SPA tests and 19 HTTP coverage tests are
verified. The original reversible negative controls failed 18 assertions before
byte-exact restoration. The added audit migration and denial-isolation regressions
also failed before their fixes, covering both CLI column orders and lifecycle
facts surviving the denial cap and retention cleanup.

| # | Control | Targeted evidence |
|---|---|---|
| 1 | Only authenticated device-user calls create/bind; internal, agent and tool posts cannot bind. HTTP and WebRTC share admission. | `app::secret_wire`, `secret_posts`, secret SQL read authority tests. |
| 2 | Upload uses existing sealed boxes, explicit domain/version and fixed 32 KiB padding; all context including grant is compared; requests cap at 64 KiB. | `secrets::upload`, JS sealing interoperability and tamper tests. |
| 3 | Plaintext/ciphertext envelopes never enter history, generic write persistence, cache or push. Public DTO is allowlisted. | staging SQL fault tests, API fixtures and metadata allowlist assertions. |
| 4 | Same request/exact sealed bytes reconciles after timeout/restart; changed content conflicts; upload/decrypt shares four-job bound. | `secrets::lifecycle::staging`, request-ID lookup tests. |
| 5 | Blob publication precedes metadata commit; failed SQL leaves inaccessible encrypted orphan; startup removes it. | staging persistence-boundary injection, existing vault crash tests. |
| 6 | Send commits complete set, message, descriptor and receipt atomically with FULL durability. Replay preserves original acceptance and TTL. | `secret_atomic`, `post_secret_claim`, failed binding rollback. |
| 7 | Existing/final provisional recipient is fenced by canonical project, owner, agent, conversation and generation. Immutable AAD stays intact. | `secret_post`, generation-race and provisional release tests. |
| 8 | Metadata and audit reads are bounded scoped snapshots; cursor scope changes are refused; tombstones and TTL remain authoritative. | `store::secrets::metadata`, malformed query/authorization tests; `environment_membership_revalidated_per_read`, archived release/create denial with metadata/safety-revoke availability and the sibling list/audit negatives. |
| 9 | Invalidation contains only IDs/revision/scope; project routing survives originating conversation retirement; lost/reordered hints reconcile without uploads. | `changes::secrets_tests`, including environment/all subscription routing, SPA `secretsWire` tests. |
| 10 | Delivery exact revision block includes scope and no values, wrapped/pinned for all five harnesses; the environment feature remains unadvertised until S2.4 (the existing 3.36 features retain their advertisements). | `agent_message::secret_tests`, API contract + withheld capability tests. |

Run only targeted gates under `nice -n 10`, with scratch CARGO_TARGET_DIR and
TMPDIR. The final task comment records exit codes, test counts, scanner results,
frozen head and merge overlap with the CLI slice. No installed bridge is run.

## S2.1 — environment grant extension (#1095)

The scoped score remains **100/100**. `secrets_contract_includes_environment_scope`
checks additive grant/selectors, strict fields, unchanged wire 3.39.0 and the
withheld environment capability. `environment_secret_upload_validates_current_stage_project_in_writer`
and `environment_secret_upload_rechecks_membership_after_opening` cover foreign
environments, archive and a membership change between opening and publication.
`environment_secret_creation_never_widens_project_grant` keeps request replay
from widening or copying an existing project grant. Create retains StageContext;
list/revoke/audit accept the environment selector. No standalone creation lifetime
is introduced. Both accepted-secret and environment-secret leak sweeps cover
values, sealed markers, live DB/WAL, metadata, pushes, harness prompts and Git,
with the existing negative controls. The frozen schema-26 fixture preserves all
old secret state through schema 27 and a second open; config repopulates the
membership projection behind the dedicated config-membership dirty fence.

### Review #1104 qualification

Old-shaped `secrets.list` requests omit environment grants, including after an
override is revoked. Only an explicit environment selector or
`include_environment:true` opts into the new DTO kind. Existing project and All
subscriptions receive project-shaped invalidations for every member project;
explicit environment subscriptions receive the environment hint. The old client's
list therefore remains parseable, while a real visibility gap remains for its
agents' readable inherited grants until that client adopts the new consumer.

`old_secret_list_excludes_environment_without_opt_in_even_after_override_revoke`
and `environment_secret_invalidations_reach_member_projects_in_legacy_shape`
verify those contracts. Fresh cryptographically sealed contexts and fresh request
IDs in `environment_secret_creation_never_widens_project_grant` demonstrate that
creating a separate environment value leaves the original project's grant and
value unchanged; a foreign grant attempt is refused without touching either.

Re-review regressions cover a non-empty legacy cursor and refusal in both
cross-mode directions (`old_secret_list_nonempty_cursor_preserves_contract_and_refuses_cross_mode`).
Membership reconciliation captures plain committed changes for every joining or
leaving project after the durable read fence clears; the off-lock flusher captures
after app unlock and enqueues truncated project hints. All subscribers receive
those hints and a legacy device refresh for an environment event with no members.
`environment_secret_interleaving_invalidates_leaving_and_joining_projects` and
`environment_secret_without_members_still_invalidates_all_in_legacy_shape` cover
the delayed-notification interleavings. Notes perform no SQL or transport work;
existing off-lock read/ack-fenced delivery and descriptor revision checks remain.

The final membership-fencing revision records actual SQL list/audit fence
refusals as bounded project/All scopes. Reconciliation captures plain committed
data only; the existing flusher captures after SQL clear and app unlock, then
enqueues off-lock. Named `device_lifecycle_tests::secrets_*` regressions cover
the exact queued-hint/unrelated-write/refused-read ordering, consecutive dirty
intervals, deterministic lock/commit ordering, independent late snapshots and
scope bounds. Subscription read/ack fencing and all wire shapes are unchanged.


Gate #1106 qualification: legacy Project/Agent/stage operations ignore registry
and membership fences. Environment operations alone consult the dedicated
config-membership fence; `include_environment:true` refuses the whole list and
records a post-clear retry. The real `settings.set` regression and retained
consecutive/late-snapshot tests exercise this fence. Real workspace-creation and
review-opening jobs held in flight allow project create/release/list/audit/revoke
and environment release/list (and create/audit/revoke).
`legacy_secret_scopes_ignore_registry_and_membership_fences` also covers legacy
agent release and staged create/list while the membership fence is dirty.

Archived projects retain project list/audit/revoke; create and release report
`project_archived`, and an archived member cannot release an environment grant.
Evidence: `archived_project_secret_metadata_and_revoke_remain_available_create_reports_archive`,
`archived_project_keeps_metadata_and_safety_revoke_but_not_release`, and
`archived_project_agent_not_current_for_secrets`.
`project_revoke_and_audit_cannot_reach_an_environment_grant` proves project
selectors refuse shared grant IDs just like unknown IDs, preserve metadata/value,
and explicit environment selectors retain audit/revoke authority.

Historical migration fixtures supply their synthetic authoritative config; the
partial-migration regression asserts the injected malformed-JSON SQLite error.
Task #1095 records the widened complete `store::` and `app::tests` results.


## S2.4 — environment settings and composer consumer (#1099)

The scoped score remains **100/100**. Wire 3.40.0 advertises
`secrets.environmentScope` with its consumer. `secretsWire.test.js` and
`secretDrafts.test.js` bind environment grants to the existing StageContext;
`environmentSettingsSecrets.test.js` covers environment-selector list/revoke/audit,
metadata-only audit projection, cached same-name overrides and one/several/no
active-member navigation. Settings never uploads or posts a message: Add secret
opens a member project's coordinator composer with This environment selected.
`secretComposer.test.js` keeps the option absent without cached support;
`agentRailDom.test.js` proves preselection without create/post calls.
`secretMetadataCache.test.js` covers separate environment/project cache keys,
explicit inherited-list opt-in, environment invalidations and retention on
membership refusal. Existing revision/generation/account fences remain in use.

The strict JS sealing helper accepts only the new environment selector shape;
`transport/js/test/secretsUpload.test.js` rejects empty, missing and mixed scope
fields. Rust `javascript_environment_grant_opens_exact_utf8_value` opens real
JS-produced environment uploads, including a maximum-size value, through S2.1's
real opener. Envelope/version/padding, storage and authority are unchanged.
`secretComposerLayout.test.js` covers the mobile environment option and its
metadata-only cache, alongside the existing full browser storage sweep. Provenance checks retain imported source
hashes and update only the root transport README's current digest.

Focused commands and final exit codes are recorded on #1099; the full suite and
Semgrep remain the serial merge gate under the task's rules.
