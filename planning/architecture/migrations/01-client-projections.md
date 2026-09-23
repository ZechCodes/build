# Client projections and presentation

**Target:** pure client selectors over cached facts. These decisions do not need an always-on process. Persisted read/watch state, history counts, access checks and workflow transitions remain authoritative in bridge services. Item numbers refer to section 1 of the #86 audit; phases are defined in the [overview](../README.md).

## Move map

| Audit item / current location | Target | Why / what stays | Dependency and rough order |
| --- | --- | --- | --- |
| 1: [board/views.rs](../../../bridge/src/app/board/views.rs), `working_time_json`, `seconds_since` | Existing [agentRailModel](../../../spa/src/core/agentRailModel.js) elapsed selector, then shared client agent/toolbar selectors | The client already derives elapsed display from `since`, falling back to bridge `seconds`. Improve clock-skew handling with a monotonic client ticker seeded from observed `seconds`; retire redundant bridge display derivation only after compatibility review. | P1 reuse existing facts and retain legacy `seconds`; P2 if adding a server observation/clock-reference field. Small. |
| 2: [app/fs.rs](../../../bridge/src/app/fs.rs), `directory_listing`, `fs_list` | Client files model and browser sheet | Directories-first sorting, ordinary dotfile visibility and labels need no background execution. Keep the current `.git` exclusion, disk enumeration, canonicalization, symlink fences, metadata protection and limits on device. | P1 with existing listings. Small. |
| 3: [tracker/notices.rs](../../../bridge/src/app/tracker/notices.rs), `reader_line`, timeline-to-notice projection | `trackerTimeline`, `trackerLineWords`, inbox selectors | Human wording changes ship with app; agent-directed notice generation remains bridge service. | P1 where structured events exist; P2 add latest-event facts where only a subtitle exists. Small/medium. |
| 4–5: [tracker/inbox.rs](../../../bridge/src/app/tracker/inbox.rs), `watched_issue_rows`; [board/views.rs](../../../bridge/src/app/board/views.rs), `board_list`, `work_items` | Client inbox/feed selectors | Membership for a view, grouping and ordering are client policy. Canonical ownership, adoption, watch state and entity identity remain bridge. | P2 raw identity/provenance, deletion and query completeness before dropping server projections. Large. |
| 6: [board/attention.rs](../../../bridge/src/app/board/attention.rs), `attention_json` | Client inbox/attention model | Badge/copy/resume display is derived. Bridge retains shared read marks, exact unread over omitted history and semantic attention facts used by notifications. | P1 display-only; P2 missing raw facts/counts. Medium. |
| 7: [tracker/mod.rs](../../../bridge/src/app/tracker/mod.rs), `issues_list`; [tracking.rs](../../../bridge/src/app/tracker/tracking.rs), `issues_for_agent` | Client filter, dashboard and assigned/tracked selectors | User-facing group membership/order belongs client. Keep existing server filtering/order and agent queries; add bounded cursor queries as a device primitive in P2, not as an assumed current capability. | P1 over complete cached lists; coordinate P2 paging with #85. Medium. |
| 8: [tracker.rs](../../../bridge/src/tracker.rs), `COLUMNS`, `normalize_status`; [edits.rs](../../../bridge/src/app/tracker/edits.rs), `optional_status/priority` | Client labels, order, explanation and form checks | Labels are presentation. Accepted status/priority values, semantic transitions and byte/fan-out limits remain validated bridge contracts. | P1 labels; do not introduce custom workflow states as incidental refactoring. Medium. |
| 9: [gitgui/unpushed.rs](../../../bridge/src/gitgui/unpushed.rs), `aggregate_work_summary`; [app/facts.rs](../../../bridge/src/app/facts.rs), `with_work_summaries` | Client workspace summary/Done display | Aggregate per-source facts for display. Keep graph walking, bounded diff/stat queries and fresh finish preflight on device. | P2 per-source facts and coverage; missing source is unknown, not clean. Medium. |
| 10, 24–25 display: [models.rs](../../../bridge/src/models.rs), catalogue labels; [harness/session.rs](../../../bridge/src/harness/session.rs), `AgentActivity::summary`; [usage_limits.rs](../../../bridge/src/app/runtime/usage_limits.rs), notices | Client picker, activity, surfaces and usage banner selectors | Labels, folding and countdowns belong client. Keep provider normalization, session provenance, observed limits, headless notices and retry decisions in bridge. | P1 existing facts; P2 structured bounded activity if summary-only data prevents desired display. Medium. |

## Use current implementations

At the source baseline, [trackerDashboardModel.js](../../../spa/src/core/trackerDashboardModel.js), [trackerAttentionModel.js](../../../spa/src/core/trackerAttentionModel.js) and [trackerIssueDetailsFeed.js](../../../spa/src/core/trackerIssueDetailsFeed.js) already derive dashboard content from cached issue/detail/activity records. Extend these selectors instead of rebuilding #84. [taskFeed.js](../../../spa/src/core/taskFeed.js) already rereads records on cache announcements. Move its product projections into the feature models while keeping a single data path.

For each row, first enumerate the exact facts the selector needs and whether each is present and complete. Add only missing facts to the protocol; implement a pure selector; compare it against representative legacy results; switch views; then retire the old projection only after compatibility review. A UI can sort one cached page, but cannot claim that it has globally ranked the tracker unless query coverage establishes that.

Do not move expensive device computation caches into the browser. [board_index/cache.rs](../../../bridge/src/app/board_index/cache.rs) avoids repeated git scans; IndexedDB avoids repeated reads and empty paints. Neither replaces the other.

## Acceptance

- Mount each affected view with seeded cache and delayed/absent network; verify the expected content.
- Exercise the actual sync -> committed cache -> notification -> reread -> render path, including an event arriving before a mutation ACK.
- Test unknown versus empty timeline/source data, clock skew, late snapshots, deletion and changed read marks.
- Keep old server fields while supported old clients consume them. No automatic action runs from a rendering selector.

**Release:** P1 app only; P2 bridge + app where facts are missing. **Prerequisite:** the [cache/contract plan](02-contracts-and-cache.md) before replacing projections whose authoritative inputs are not yet available.
