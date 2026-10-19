# Compaction editors security checklist

Scope: #661, SPA scoped settings, conversation menu and context gauge. Wire,
policy persistence and harness enforcement remain covered by the #660 and
session-policy checklists. Each control is worth 10 points. **Score: 100/100.**

| # | Control | Evidence |
| --- | --- | --- |
| 1 | Device, project and workspace edits use the existing adapter and exact owner IDs; cached support cannot authorize new RPC fields | `scopedCompactionEditor.test.js` scope setter cases; `deviceContexts.test.js` current greeting and downgrade; `compactionContract.test.js` older-bridge refusal |
| 2 | All new values are positive safe integers; zero, fractions, negatives, nonnumeric input and unsafe integers are refused | Editor validation and conversation choice tests; no Off/Never menu option |
| 3 | Unsafe legacy decimal observations remain exact read values, never rounded into writable numbers | Editor decimal/unsafe tests and conversation custom-limit tests |
| 4 | Only authoritative replies or invalidation reads write settings cache; failures cannot publish drafts | Editor failed-save/cache assertions; no optimistic settings mutation |
| 5 | Replies cannot replace intervening cache writes or push invalidations | Editor stale-reply and invalidation-fence tests; `compactionAnswerCache.test.js`; existing #660 compaction cache/race suites |
| 6 | Cold views and later greetings read cached records without transport-status branches; no compaction setting is stored in localStorage | Three settings surface cache tests; editor mount sends no RPC; browser module tests |
| 7 | Removing a surface retires subscriptions and pending save authority; repainting retained sections preserves their ownership | Editor removal/transplant tests and project settings integration regression |
| 8 | Draft input, focus and selection survive cache and greeting updates; keyboard and narrow layouts remain usable | Editor DOM suite, settings integration regressions, Chromium scoped-editor and menu-continuity suites |
| 9 | Gauge denominator comes only from applied active policy or a known model window; pending configuration cannot masquerade as active enforcement | `contextGauge.test.js`, menu observation tests and Chromium gauge-state captures; applied-at-start/fallback/unknown labels |
| 10 | Untrusted values and errors enter textContent/value or escaped markup; changes introduce no credentials, subprocesses, network policy or live bridge operations | Editor safe error-rendering tests; existing escape helpers; complexity ratchet, Semgrep, gitleaks and diff gates in #661 handoff ledger |

The UI does not claim that saving restarts work or changes the current session.
Clearing one override sends provider null (or conversation null) and lets the
bridge resolve the inheritance chain. The legacy configured scalar is not
used as evidence of the active compaction threshold. Unknown native triggers
use the model window/current usage rather than a guessed countdown.
