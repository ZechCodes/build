# Build v1 — UI Design Brief & User Stories

**Product:** Build (getbuild.ing)
**Companion to:** Build v1 Scope Document
**Status:** Draft for design
**Last updated:** June 11, 2026

---

## 1. The Experience in One Paragraph

Build should feel like reviewing work, not watching work. The user's time goes to two surfaces — a plan they're shaping and a diff they're judging — connected by stretches where Build is silent and the user is living their life. The terminal exists everywhere and announces itself nowhere: one gesture away from any screen, never the default view, never required for the happy path. If the design is right, a user ships a feature from their phone at the playground without ever seeing a terminal — and a power user who *wants* the terminal never feels it's been hidden from them.

---

## 2. Design Principles

**1. Review is the product.** Plan review and diff review are where the user spends attention, so they get the richest interactions, the best typography, and the most design effort. Everything else is plumbing to get the user to a review or away from one.

**2. The terminal is the basement.** It's always there, it holds everything (it *is* the log), and you go down when something needs fixing — not to hang out. Access must be instant and consistent from every task context (one gesture/keystroke, same one everywhere). Presence must be invisible: no terminal previews on cards, no output snippets in notifications, no "see what your agent is doing live!" affordances.

**3. Calm by default.** Build's promise is *start work and walk away*. The UI must not punish walking away or reward staring. No streaming text, no spinners that imply you should wait, no live tool-call feeds. While an agent works, Build shows a quiet, glanceable progress state — and actively communicates "you can leave."

**4. Artifacts, not transcripts.** Every interaction attaches to a thing: notes attach to the plan, comments attach to diff lines, decisions attach to cards. There is no freestanding conversation anywhere in the UI.

**5. Glanceable from a phone, deep on a desktop.** The phone is for state, decisions, and light review (approve a plan, skim a small diff, answer a blocked card). The desktop is for heavy review. Same surfaces, responsive depth — not two products.

**6. Legibility is the security model.** v1 has no permission system; what it has is honesty. The UI's job is to make what agents did completely visible (the diff is total) and what agents *could* do plainly stated (YOLO disclosure at onboarding and dispatch).

---

## 3. Information Architecture

```
Board (home)
 ├─ Task ───────────────┐
 │   ├─ Plan tab        │  ← the two core surfaces
 │   ├─ Diff tab        │
 │   └─ Terminal (overlay/drawer — not a peer tab visually; see §4.4)
 ├─ New Task (dispatch sheet)
 ├─ Notifications
 └─ Settings
     ├─ Devices & keys (pairing, fingerprints)
     ├─ Projects (repos, merge behavior, templates)
     └─ Harnesses (detected credentials, integration mode defaults)
```

Navigation depth is deliberately shallow: nothing the user needs is more than two taps from the board.

---

## 4. The Surfaces

### 4.1 Board (home)

The board answers one question: **what needs me, and what doesn't?**

- Tasks grouped by *attention state*, not lifecycle state. Two buckets, in this order:
  - **Needs you** — plan_review, review, blocked, failed, idle_unreported
  - **Working** — planning, building (and a collapsed "Done" section: merged/abandoned)
- A task card shows: title/goal, project + branch, phase chip, time-in-state, and a one-line payload (the `done` summary, the blocked reason, or for working tasks a quiet progress fact: "7 files changed · last activity 2m ago").
- Cards in **Needs you** are visually loud (accent color, top of screen). Cards in **Working** are visually quiet — almost greyed. The design should make an empty "Needs you" bucket feel like an achievement: *nothing needs you, go live your life.*
- No terminal previews, no streaming text on any card.
- Primary action on the board: **New Task** (prominent, fast).

### 4.2 Dispatch (new task)

A sheet, not a page — dispatching should feel lightweight:

- Goal (multiline text, the main event)
- Project/repo, base branch (remembered defaults)
- Harness + credential chip — shows detected class inline: "Claude Code · Max plan (plan limits)" with a tap-to-expand detail panel (mode, billing bucket, re-check). Defaults do the steering; details are one tap away; override is never buried.
- **Quick task toggle** — skips the plan phase for trivial work (goal → build → review). The toggle copy should set expectations: "Skips planning. Best for small, unambiguous changes."
- One YOLO disclosure line with an info link (first dispatch per project expands it fully; after that it's a quiet footer line). Honest, not nagging.
- Submit → card appears in **Working · planning**. The sheet closes immediately. Nothing asks the user to wait.

### 4.3 Task view — the two core tabs

#### Plan tab

The plan is a rendered document (`.build/plan.md`), and this surface should feel like reviewing a document someone handed you — closer to a Google Doc in suggestion mode than a code review:

- Clean reading typography. The plan is the hero; chrome recedes.
- **Notes attach to selections.** Select any passage → note composer anchors to it. General notes attach to the whole document.
- Notes accumulate in a local pending tray (visible count, editable, deletable) and submit as **one batch**: a single "Send notes" action dispatches the revision round. No note is ever sent individually.
- After a revision, changed sections are marked (a subtle gutter indicator — "changed since your notes") so re-review is a skim, not a re-read.
- The approve action is singular and weighty: **Approve plan & start build.** This is the dispatch tap; it should feel like a decision, not a dismissal. Confirmation shows what happens next ("A fresh session will execute this plan").
- If the plan phase touched files outside `.build/` (enforcement by observation), a flag banner sits at the top of this tab: "The planning agent also modified 3 files — view diff." Informational tone, not alarm.
- Empty/working state (while planning): the goal text, a quiet "drafting plan" indicator, time elapsed, and an explicit "You'll be notified — no need to wait." No streaming.

#### Diff tab

This is the PR-review surface, designed to be the best one the user has used:

- **While building:** not a watchable stream. A progress summary only — files changed count, additions/deletions, last-activity timestamp, current phase. One line of design intent: this screen should be *boring on purpose*, and say so ("Building. You'll be notified when it's ready for review.").
- **At review:** the full diff materializes. File tree sidebar (desktop) / file accordion (mobile), per-file viewed-checkmarks, syntax highlighting, collapsible unchanged regions.
- **Comments attach to lines or files**, accumulate in the same pending-tray pattern as plan notes, and submit as one batch via **Request changes** (with anchors: `file:line` references travel with each comment).
- Two terminal review verdicts, mirrored from the plan tab: **Request changes** (sends the batch) and **Approve & merge** (runs the git ops; shows the merge target per project settings).
- After a correction round: changed-since-last-review markers, same as the plan tab. The two review surfaces must share one interaction grammar — learn it once.
- The plan is reachable from the diff (split view on desktop, link on mobile): reviewers judge the diff *against the plan*, so the plan is reference material here.

### 4.4 Terminal — out of sight, always accessible

The terminal is **not a peer tab**. It's an overlay/drawer summoned from anywhere within a task:

- **Desktop:** a keystroke (e.g., backtick, configurable) slides it up as a bottom drawer over the current tab, full xterm.js attached to the live PTY. Same key dismisses. Resizable, can pop to full screen.
- **Mobile:** a persistent but quiet affordance (e.g., a small grip/handle at the bottom edge of the task view) swipes up into a full-screen terminal. Swipe down to dismiss.
- The terminal is *the log*: scrollback holds the session history. No separate activity log UI exists anywhere.
- Attaching never interrupts the agent; it's a window into the same PTY. Typing into it is first-class — this is the deep-interaction surface.
- Discoverability without presence: onboarding mentions it once ("Every task has a live terminal — swipe up / press ` anytime"), and blocked/idle_unreported cards include "Open terminal" as one of their resolution actions. Otherwise the UI never advertises it.

### 4.5 Notification cards

Notifications are decisions delivered to the user, so they carry payloads and actions, not just pings:

| Trigger | Card content | Actions |
|---|---|---|
| Plan ready (`done`, phase=plan) | Goal + agent's summary | Review plan |
| Build ready (`done`, phase=build) | Summary + diff stats | Review diff |
| Blocked (`done`, status=blocked) | The agent's reason, verbatim summary | Open plan · Reply (follow-up prompt) · Open terminal · Abandon |
| Failed (`done`, status=failed) | What didn't work | Same as blocked |
| Idle, unreported (quiescence) | "Agent went quiet without reporting done" + last-activity time + diff stats | View diff · Open terminal |
| Merge complete | Branch → target confirmation | View task |

Push notifications mirror the card title + one-line payload (content travels encrypted; rendered client-side). Tapping a push deep-links to the relevant surface, not to the board.

### 4.6 Settings (the trust surfaces)

- **Devices & keys:** paired devices, key fingerprints displayed for verification, revoke. This page is part of the product's pitch — design it to be shown off, not buried. Plain-language framing: "Build's servers move ciphertext. These keys are why."
- **Harnesses:** detected credentials per harness with class and billing bucket, versioned detection timestamp, re-check button. The "what these credentials can do keeps changing upstream" reality should be visible here, calmly.
- **Projects:** merge behavior (merge to base / push branch / open PR), template overrides (links to `.build/templates/` files — the UI doesn't edit them in v1, it documents them).

---

## 5. User Stories

Format: story, then acceptance criteria (AC). Grouped by journey. "User" is a developer with build-bridge running on their own machine and at least one harness authenticated locally.

### Journey A — Dispatch & walk away

**A1.** As a user with an idea on my phone, I want to create a task by typing a goal and tapping submit, so that work starts without me being at my desk.
- AC: From the board, dispatch completes in ≤ 3 interactions (new task → goal → submit) using remembered defaults.
- AC: The sheet closes immediately on submit; the task appears under Working with a "drafting plan" state.
- AC: Nothing in the post-dispatch UI implies I should wait or watch.

**A2.** As a user dispatching a trivial change, I want a quick-task option that skips planning, so that "fix the typo in the README" doesn't get a ceremony.
- AC: Quick task goes goal → building directly; the task view shows no Plan tab (or shows the goal in its place).
- AC: The toggle explains the tradeoff in one line.

**A3.** As a user with both a Max plan and an API key, I want to choose which credential a task runs on at dispatch, with the billing bucket plainly labeled, so that I control my own spend.
- AC: The credential chip shows harness + detected class + bucket ("Max plan — plan limits" / "Console key — metered").
- AC: Detail panel exposes the full disclosure and a re-check action; override persists per project.

**A4.** As a first-time user, I want the YOLO reality stated plainly before my first dispatch, so that closing my laptop on an agent is a choice I made knowingly.
- AC: First dispatch per project shows the expanded disclosure (worktree ≠ sandbox; what agents can reach); subsequent dispatches show a one-line footer with an info link.
- AC: The disclosure is honest and calm — no scare styling, no legal-wall feeling.

### Journey B — Plan review (core)

**B1.** As a user, I want to be notified when a plan is ready and read it as a clean document, so that my first interaction with the work is judgment, not monitoring.
- AC: Push notification carries the agent's `done` summary; tapping it lands directly on the Plan tab.
- AC: The plan renders with document-grade typography; no terminal output, no tool-call noise anywhere on the surface.

**B2.** As a user reading a plan, I want to attach notes to specific passages and send them all at once, so that the agent gets one coherent revision request instead of a drip-feed.
- AC: Select-to-note works on any passage; general notes are also possible.
- AC: Pending notes are visible, editable, and deletable before sending; one "Send notes" action submits the batch.
- AC: After sending, the task moves to Working state and tells me I'll be notified.

**B3.** As a user re-reviewing a revised plan, I want to see what changed since my notes, so that re-review is a skim.
- AC: Changed sections carry a gutter marker; a "changes only" view filter exists.

**B4.** As a user satisfied with a plan, I want a single decisive approve action that starts the build, so that approval and dispatch are one moment.
- AC: "Approve plan & start build" requires one confirm; the confirm states that a fresh session will execute the plan.
- AC: Task transitions to Working · building; I'm explicitly told I can leave.

**B5.** As a user whose planning agent wrote code it shouldn't have, I want to be told without drama, so that I can decide whether it matters.
- AC: An informational banner on the Plan tab links to the out-of-scope diff; no blocking, no alarm styling.

### Journey C — While it builds

**C1.** As a user waiting on a build, I want the task to show quiet progress facts rather than streaming output, so that I don't get sucked into watching.
- AC: Building state shows files-changed count, +/- lines, last-activity time, elapsed time. Nothing animates continuously.
- AC: No live diff text or terminal output renders on this surface.

**C2.** As a user away from all my screens, I want the system to need nothing from me until a gate, so that "walk away" is real.
- AC: Between approve and review-ready, zero interactions are required for the happy path.

### Journey D — Diff review (core)

**D1.** As a user notified that a build is ready, I want to review the complete diff against the plan, so that I judge the work in one sitting with full context.
- AC: Review state presents the full diff (file tree/accordion, syntax highlighting, viewed-marks per file).
- AC: The plan is one interaction away (split view on desktop; link on mobile).

**D2.** As a reviewer, I want line- and file-anchored comments that batch into one "Request changes," so that the agent receives a coherent change request with exact anchors.
- AC: Same pending-tray grammar as plan notes; submit sends one prompt with `file:line` anchors.
- AC: The session receiving the batch is the warm build session (corrections land in context).

**D3.** As a re-reviewer, I want changed-since-my-review markers, so that correction rounds get cheaper, not more tiring.
- AC: Files/hunks changed since the last review round are marked; "changes only" filter exists.

**D4.** As a satisfied reviewer, I want approve-and-merge to run the git ops and confirm the result, so that done means merged.
- AC: The action names the merge target per project settings; success notification confirms branch → target; failure surfaces the git error with "Open terminal" as a resolution.

**D5.** As a reviewer on my phone with a small diff, I want review to be genuinely workable one-handed, so that small tasks ship from anywhere.
- AC: Mobile diff supports per-file collapse, comment composition, and both verdict actions without horizontal scrolling pain on a phone-width viewport.

### Journey E — When it doesn't go smoothly

**E1.** As a user whose agent is blocked, I want its reason delivered as an actionable card, so that unblocking takes one decision, not an investigation.
- AC: Blocked card shows the agent's summary verbatim plus actions: open plan, reply (follow-up prompt composer), open terminal, abandon.
- AC: Reply writes a human-composed follow-up into the warm session and returns the task to Working.

**E2.** As a user whose agent went quiet without reporting done, I want that flagged as its own thing — not as success — so that I check before trusting.
- AC: idle_unreported is visually distinct from review-ready everywhere (board, card, push).
- AC: Card offers: view diff, open terminal.

**E3.** As a user who needs to intervene directly, I want the terminal one gesture away from wherever I am in the task, so that deep interaction is instant when I want it and invisible when I don't.
- AC: Same gesture/keystroke summons the terminal from Plan, Diff, and any card's "Open terminal" action.
- AC: The terminal attaches to the live PTY with full scrollback; typing works; dismissing returns me exactly where I was.
- AC: No surface outside the terminal ever renders terminal output.

**E4.** As a user abandoning a task, I want the worktree cleaned up and the branch kept, so that abandoning is safe and reversible-ish.
- AC: Abandon confirms once, states what's kept (branch) and removed (worktree, sessions); the task moves to Done · abandoned.

### Journey F — Parallel work & the board

**F1.** As a user running several tasks, I want the board to separate "needs me" from "working," so that a glance tells me whether I'm needed at all.
- AC: Two-bucket layout; Needs-you items sort above and read louder than Working items.
- AC: An empty Needs-you bucket has a designed state that affirms it ("Nothing needs you").

**F2.** As a user running two tasks on the same repo, I want them fully independent in the UI, so that parallelism feels safe.
- AC: Each task shows its own branch; nothing in the UI crosses task boundaries except the project grouping.

**F3.** As a user juggling tasks across harnesses, I want each card to show which harness/credential it's on, so that mixed fleets stay legible.
- AC: Harness chip on cards and task headers.

### Journey G — Trust & setup

**G1.** As a new user pairing my phone, I want QR pairing with visible key fingerprints, so that the E2EE claim is verifiable, not vibes.
- AC: Pairing flow completes phone-to-bridge with a fingerprint comparison step; Devices & keys lists every paired device with its fingerprint and a revoke action.

**G2.** As a security-conscious user, I want the UI to tell me what the relay can and can't see in plain language, so that I can explain it to someone else.
- AC: A "what our servers see" panel exists in settings: routing IDs, ciphertext sizes, timing — and explicitly nothing else.

**G3.** As a user whose vendor keeps changing the rules, I want credential detection to be visibly dated and re-checkable, so that a stale assumption never surprises me.
- AC: Detection shows "checked <time> ago" + re-check; mode defaults update on re-check with a notice if the class changed.

---

## 6. Interaction Grammar (shared patterns)

These patterns repeat across surfaces and must be designed once, consistently:

1. **The pending tray.** Notes (plan) and comments (diff) accumulate locally, visibly, editably — then submit as one batch. One grammar, two surfaces.
2. **The decisive gate.** Approve-plan and approve-merge are the two weighty actions in the product. Same visual weight, same confirm pattern, same "here's what happens next" framing.
3. **The changed-since marker.** Plan re-review and diff re-review both mark deltas since the user's last round, with a changes-only filter.
4. **The card with actions.** Every notification is a payload plus its resolution actions. No dead-end pings.
5. **The summon gesture.** One gesture/keystroke for the terminal, identical everywhere.
6. **The quiet working state.** Every in-progress surface shares the same visual language: facts, timestamps, "you'll be notified," nothing streaming.

---

## 7. Mobile vs. Desktop Posture

| | Phone | Desktop |
|---|---|---|
| Board | Primary surface; triage and dispatch | Same, denser |
| Plan review | Full capability — reading + notes are phone-native | Same + side-by-side with diff |
| Diff review | Workable for small/medium diffs; viewed-marks and collapse carry the weight | The heavy-review home; split plan/diff |
| Terminal | Full-screen overlay; usable, not optimized | Drawer; fully capable |
| Notifications | The main entry point into the app | Secondary |

The phone story we're designing for: *triage on the board, approve a plan in line at the store, answer a blocked card from the couch, review a small diff at the playground, drop into a terminal only when something's weird.*

---

## 8. States Checklist (for design completeness)

Every surface needs designed states for:

- Board: empty (no tasks ever — first-run), all-quiet (nothing needs you), mixed
- Task: planning, plan_review (incl. out-of-scope flag variant), building, review, re-review (changed-since markers), blocked, failed, idle_unreported, merging, merged, abandoned
- Diff: empty (no changes yet), small (single file), large (50+ files — collapse/virtualization strategy needed)
- Terminal: attached/live, session released (warm-cap LRU or post-merge — scrollback preserved, "session ended" banner)
- Connectivity: bridge offline (laptop asleep — the honest state: "your machine is unreachable"; tasks show last-known state, clearly stamped)
- Notifications: granted/denied permission states (denied needs an in-app inbox to carry the load)

The **bridge-offline** state deserves real design attention: it's the inherent cost of the local-custody architecture and the moment users compare Build to cloud agents. It should be honest, calm, and specific ("Your machine has been unreachable since 3:42 PM — tasks will resume when it reconnects"), never error-red.

---

## 9. Out of Scope for This Design Pass

- Visual identity / brand system for Build (separate effort; this brief is structure and behavior)
- SDK-mode and auto-dispatch surfaces (v1.x — but the credential chip and mode switch are designed now to receive them)
- Permission/approval cards (v2 — note they will reuse the card-with-actions pattern)
- Template editing UI (v1 documents the files; doesn't edit them)
- Team/multiplayer anything
