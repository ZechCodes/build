# Build v1 — Scope Document

> **Amendment (2026-09-18):** `post_thread_message` takes only `status`,
> `body` and `options`. There is no `phase`, `outputs`, `anchor` or `links`:
> Build knows which phase a report closes from the session that sent it, and a
> plan's stages are read from `.build/plan/stages.json` on disk when the plan
> agent reports Complete. The per-stage validation gate (validate/fix-stage
> sessions, `ValidationReport`, `run.stage_fix`/`issue.stage_fix`), diff triage
> (`triage.override`, `triage_enabled`, `.build/review-rules.json`), the
> branch-recovery agent (`RecoveryAttempt`, `phase=recover`) and agent-reported
> comment resolutions are removed. A stage is `building` until its build
> reports Complete, then `completed`. Where this document says otherwise, this
> note wins.

**Product:** Build (getbuild.ing)
**Status:** Draft for review
**Last updated:** June 11, 2026

---

## 1. What v1 Is

**Build v1 is tmux for coding agents: in your browser, end-to-end encrypted, with git-native review.**

You set a goal from any device. An agent on your hardware writes a plan. You review the plan, leave notes, approve. An agent builds. You review the diff, comment, approve. Build merges. At any point you can drop into the agent's terminal — but you never have to.

Build does not run agents. It does not host code. It does not see code. Agents run on the user's own machine via `build-bridge`; the relay moves ciphertext and nothing else. v1's job is **orchestration**: starting work, watching it through git, and gating the transitions where human judgment matters.

### Positioning

The market splits into local orchestration dashboards (Conductor, Vibe Kanban — your hardware, but you're chained to your desk) and cloud agents (Devin, Codex Cloud, Jules — go anywhere, but your code lives in a vendor VM). Build is the only product in the quadrant they both miss: **cloud-agent convenience with local-agent custody.** Your agents run on your hardware; our relay can't read a byte of it.

---

## 2. Scope

### In scope for v1

- Task-based orchestration on top of git worktrees (worktree-per-task)
- The five-phase lifecycle: goal → plan → approve → build → review → merge
- Agents spawned as interactive CLI sessions in PTYs, in YOLO mode (`--dangerously-skip-permissions` or harness equivalent)
- Embedded terminal per task (xterm.js) streamed over the existing E2EE relay
- Git-powered diff view and batched review comments
- The Build MCP server with a single `done` tool
- User-editable prompt templates as files
- Push notifications on phase transitions and idle detection
- Credential-class detection (subscription / credit / API key) with the integration-mode switch defaulted from it
- Multi-harness support via the trivial v1 adapter contract (spawn command + PTY write)

### Explicitly out of scope for v1 (deferred, not rejected)

| Deferred | Target | Why it's out |
|---|---|---|
| Hooks + transcript ingestion | v1.x | Richer state source; git diff is sufficient for v1 |
| SDK / stream-json mode | v1.x | Second input path; PTY mode works for both billing postures |
| Auto-dispatch, loops, scheduled tasks | v1.x | Requires SDK mode + credit/key auth to do cleanly |
| Permission system / approval gates | v2 | The whole point of v1 is start work and walk away |
| Containers / sandboxing / egress control | v2 | OS-level isolation layer wraps the same spawn call later |
| `ask_user` and other MCP tools beyond `done` | v2 | `done(status=blocked)` covers the v1 need |
| Plan-mode wrapping, activity log UI | Never (probably) | The terminal remains the execution log; the dedicated Conversation tab carries only durable user/agent messages and lifecycle statuses |

### Design rules

1. **Everything is files + git + PTY.** Every phase is an agent in a PTY operating on files. Build prompts, watches git, and gates transitions. No harness internals, no hooks, no scraping TUI output for state.
2. **Enforcement by observation, not permission.** v1 never restrains an agent. It makes what the agent did legible (git diff) and lets the human decide.
3. **Build is not the policy police.** Users are fully in control with all relevant details. Defaults steer; disclosures inform; the user decides.
4. **Nothing in v1 gets thrown away.** Hooks slot in as a richer state source on the same PTY sessions. Permissions arrive as interception on the hook layer. Containers wrap the same spawn call. The MCP server grows tools.

---

## 3. Architecture

Three components, two pipes, one source of truth.

```
┌─────────────┐   E2EE relay    ┌──────────────┐   spawns    ┌──────────────┐
│  Web client │◄───ciphertext──►│ build-bridge │────PTY─────►│ agent harness │
│  (browser)  │                 │ (user's box) │◄────MCP─────│  (worktree)   │
└─────────────┘                 └──────┬───────┘             └──────────────┘
                                       │ watches
                                       ▼
                                  git worktree
```

- **Web client** — task board, embedded terminals, diff/review surface, notifications. Decrypts everything client-side.
- **Relay** — moves ciphertext between client and bridge, and after the WebRTC upgrade carries only signaling, presence and fallback (see section 8). Sees routing IDs, lengths, timing. Nothing else. (Already built.)
- **build-bridge** — the daemon. Owns worktrees, spawns harnesses in PTYs, serves the Build MCP server, watches git, runs git operations, talks to the relay.

### The two pipes

- **Build → agent: the PTY.** Dispatching a phase means writing a prompt into the agent's stdin. Review feedback means writing a batched comment prompt. The user dropping in means attaching their terminal to the same PTY. There is exactly one way to talk to an agent.
- **Agent → Build: the MCP server.** The agent reports completion (and only completion, in v1) by calling the `done` tool. There is exactly one way for an agent to talk to Build.

### The state source

**Git is the integration layer.** Build never asks the harness what it did — the worktree knows. The bridge watches the worktree and renders `git diff` against the base branch. This cannot drift from reality, requires zero harness integration, and works identically for every CLI harness. v1 is multi-harness as a side effect.

### The harness adapter contract (v1)

An adapter is two things:

1. A spawn command (binary, YOLO flag, MCP config mechanism)
2. Knowledge of how to write a prompt to its PTY

That's the whole contract. Claude Code, Codex, and OpenCode all fit.

---

## 4. Task Model & Lifecycle

A **task** is: a goal, a worktree + branch, a sequence of phase sessions, and a state.

```
created ──► planning ──► plan_review ──► building ──► review ──► merged
                │  ▲                        │  ▲          │
                │  └── notes (batch) ───────┘  └─ changes ┘
                └────────────── abandoned (from any state) ─┘
```

### Phase walkthrough

**Created.** User submits a goal (and picks harness + credential if they have more than one). Bridge creates the worktree and branch, writes the task's `.mcp.json` pointing at `build-bridge mcp --task <id>`, and stages the plan prompt.

**Planning.** Bridge spawns the harness in a PTY and writes the plan prompt from the template: produce an implementation plan at `.build/plan.md` for the goal — *plan only, no implementation* — and call `done` with the plan path when finished. Enforcement by observation: if the diff during this phase touches anything outside `.build/`, the UI flags it.

**Plan review.** The plan is a file, so it gets the same review surface as code. The user reads the rendered plan, leaves notes. Notes **batch** — they accumulate and submit as one revision prompt with anchors, like a PR review. The planning session stays warm so revisions land in context. Approve advances the phase.

**Building.** Approve spawns a **fresh session** (default) with the build prompt: execute `.build/plan.md`, call `done` when complete or blocked. Fresh-by-default is deliberate: if a cold agent can't execute the plan, the plan wasn't done. It also makes the plan a durable handoff — plan with one harness or model, build with another. (Same-session continuation is a per-task option for users who want it.)

**Review.** The diff materializes in full at this gate (during the build it's a progress indicator — files changed, last activity — not a watchable stream). The user comments on lines/files; comments batch; "request changes" submits one coherent instruction to the still-warm build session. Approve advances.

**Merged / abandoned.** Merge runs the git ops (merge to base, or push branch — per-project setting). Abandon removes the worktree, keeps the branch. Either way, all sessions for the task are released.

### Session lifetime policy

A phase's session stays **warm from dispatch until its review gate passes** (or the task is abandoned). Warm sessions are what make revision rounds cost thirty seconds instead of a cold re-read of the codebase. The bridge owns the resource policy (cap on concurrent warm sessions, LRU release with notification) since N warm agents on a laptop is real memory.

### The plan file

- Lives at `.build/plan.md` in the worktree.
- **Committed and kept through merge.** The plan is the intent record — six months later, "why does this code exist" has an answer. ("Intent as infrastructure," literally.)
- The plan agent reports its location in the `done` payload, so the path is convention, not hardcode.

### Prompt templates

Prompt templates **are** the orchestration logic, so they're user-editable files, not strings in code:

```
.build/templates/plan.md
.build/templates/build.md
.build/templates/revise.md
.build/templates/review-changes.md
```

Bridge ships defaults; a project can override any of them. The `done`-tool instructions live in the templates (harness-agnostic, user-overridable, zero special cases). Template variables: `{goal}`, `{plan_path}`, `{comments}`, `{base_branch}`.

---

## 5. Definition of Done

"Done" has a precise meaning in Build. This section is normative.

### 5.1 The `done` tool

The Build MCP server (spawned per-session over stdio, task identity baked into the transport — no shared server, no auth, no ambiguity) exposes exactly one tool in v1:

```json
done {
  "phase":   "plan" | "build" | "revise",
  "status":  "completed" | "blocked" | "failed",
  "summary": "string — one paragraph, what was done or why it stopped",
  "outputs": {
    "plan_path": "string — required when phase=plan and status=completed"
  }
}
```

Every phase prompt template instructs the agent: *when you complete this phase, call `done` with status and outputs; if you cannot proceed, call `done` with status `blocked` and explain why in the summary.*

### 5.2 What each status means

**`completed`** — The agent asserts the phase objective is met. This is a *claim*, not a verdict: completion of a phase never advances a task past a human gate. `done(completed)` on a plan means "plan ready for your review." `done(completed)` on a build means "diff ready for your review." The human gates are the actual definition of done for the task.

**`blocked`** — The agent cannot proceed and is telling you why (ambiguous spec, contradictory constraints, missing credentials, failing environment). This is a first-class outcome, not a failure: it gives agents a sanctioned way to stop and report instead of flailing or hallucinating through ambiguity. Renders as an actionable notification card with the summary. Resolution paths: revise the plan, answer via a follow-up prompt, drop into the terminal, or abandon.

**`failed`** — The agent attempted the phase and asserts it did not succeed (e.g., could not make tests pass within the plan's approach). Same surfaces as blocked, different semantics: blocked means "I need something," failed means "this approach didn't work."

### 5.3 Per-phase done criteria

| Phase | The agent's claim when calling `done(completed)` | Verified by |
|---|---|---|
| Plan | A complete, self-contained plan exists at `outputs.plan_path`; a cold agent could execute it | Human, at plan review |
| Build | The plan at `.build/plan.md` is implemented in the worktree | Human, at diff review |
| Revise | The batched notes/comments have been addressed | Human, at re-review |

| Task state | Definition of done |
|---|---|
| Plan approved | Human approved the plan file as reviewed |
| Task done | Human approved the diff and the merge git ops succeeded |

### 5.4 Fallback: quiescence, and what it does *not* mean

Agents forget tool instructions. Context compaction eats system prompts. Sessions crash. So done-detection is belt and suspenders:

- **`done` called** → definitive. Task transitions with the payload.
- **PTY quiet for N seconds with no `done`** → the task enters **`idle_unreported`** — a distinct state, *not* treated as completion. In practice it usually means stuck, crashed, or awaiting something. Notification: "agent went quiet without reporting done." The user glances at the diff or drops into the terminal.

The `done` tool is the protocol; quiescence is demoted from primary signal to anomaly detector. Both are cheap. (Hooks in v1.x upgrade idle detection further without touching anything else.)

### 5.5 What "done" is *not*, in v1

- Not "tests pass." v1 has no opinion on verification beyond what the plan specifies and the human checks. (Automated check-runs at the review gate are a v1.x candidate.)
- Not "merged." Merge is a human act plus git ops; `done` never merges anything.
- Not a permission boundary. v1 agents run YOLO; `done` is reporting, not control.

---

## 6. Component Specs

### 6.1 build-bridge (daemon)

- Worktree lifecycle: create on task creation (`git worktree add`), remove on merge/abandon, branch naming convention `build/<task-slug>`
- PTY management: spawn harness per phase session, hold warm per the session-lifetime policy, attach/detach user terminals, write prompts
- Diff watcher: fs-watch the worktree, debounce, compute diff vs base branch, push encrypted diff summaries (files changed, last activity) continuously and full diffs at review gates
- Build MCP server: stdio, per-session, `--task <id>`, one tool (`done`)
- Git operations: merge, push, branch cleanup — executed locally, results reported through the relay
- Credential detection: per-harness check of credential class (subscription / credit / API key), surfaced to the client with a re-check action — **detection is versioned and re-checkable** because what these credentials are allowed to do keeps changing upstream
- Quiescence timer per PTY

### 6.2 Web client

- **Task board** — tasks by state; created/planning/plan_review/building/review/idle_unreported/blocked badges
- **Task view** — three tabs: Plan (rendered `.build/plan.md` + note composer), Diff (progress indicator while building; full diff + line comments at review), Terminal (xterm.js attached to the live PTY — the log lives here, the escape hatch lives here)
- **Review composer** — comments accumulate locally; one "submit notes" / "request changes" action per round
- **Integration-mode switch** — per task: mode (PTY now; SDK greyed "coming v1.x"), credential in use, billing bucket it draws from ("Detected: Max plan — plan limits" / "Console key — metered"), defaulted from detection, freely overridable, details on the label
- **Notifications** — phase transitions, `done` payload summaries, blocked/failed cards, idle_unreported
- All decryption client-side; key fingerprint verification surfaced in settings

### 6.3 Relay

Already built. v1 adds no relay features. (The streaming-crypto upgrade — pairing + `crypto_secretstream`, per-task subkeys — rides alongside v1 since terminals are long-lived streams; sealed boxes remain for one-shot messages.)

---

## 7. Auth & Billing Posture

- Build never handles Anthropic (or any vendor) credentials. Users authenticate harnesses locally (`claude /login`, API keys in their own env). Model traffic goes direct from the user's machine to the vendor; the relay never carries it.
- The integration-mode switch is named by **mechanism** (PTY session vs. SDK), not by compliance posture. Build's job is to tell the user what each mode does and which billing bucket it draws from. The user's relationship with their vendor is their own.
- In v1, every prompt an agent receives is human-initiated: a dispatch, an approval, a batched review round, or terminal input. There is no Build-originated prompting in v1 — not as policy enforcement, but because auto-dispatch is deferred to the SDK path in v1.x where it belongs technically.
- Defaults steer: subscription-detected → PTY mode (the only v1 mode anyway). When SDK mode ships, key/credit-detected defaults there for automation features. Overrides always available, always with the relevant details displayed.

---

## 8. Security Posture & Known Limitations (v1)

State this plainly in the docs and the onboarding flow — transparency is the brand:

- **Agents run in YOLO mode with no sandbox.** A worktree isolates *branches*, not the machine. An agent (or a prompt-injected agent) can read anything the user can read and reach any network the machine can reach. Indirect prompt injection via web content is an in-the-wild attack class, not a hypothetical.
- **What v1 does provide:** the E2EE relay (Build's infrastructure is not a party to your code, prompts, or diffs), branch isolation (no agent touches main), and legibility (the diff shows everything that changed in the worktree).
- **What v2 adds:** per-task permission profiles, PreToolUse approval gates on the hook layer, container/namespace isolation wrapping the same spawn call, egress allowlists. None of it requires rearchitecting v1.
- **A second infrastructure party.** Browser and bridge negotiate a direct WebRTC DataChannel and use Cloudflare TURN only when neither peer can hole-punch, which makes Cloudflare a second infrastructure party beside the relay. Cloudflare sees TURN allocation source IPs and DTLS ciphertext; under that DTLS is the same secretbox envelope the relay carries, so even a broken DTLS session exposes no more than the relay already sees — session ids, sizes, timing — and never plaintext or session keys. The peer's DTLS fingerprint travels inside the sealed session, so neither Cloudflare nor anyone else on the path can substitute a peer. The direct path adds the one exposure the relay path hid: each peer learns the other's IP. TURN credentials are short-lived — their lifetime is `TTL_SECONDS` in `skriftapp/buildapp/ice_servers.py` — minted per authenticated user by the api, and reach the bridge inside the sealed session; the TURN key itself never leaves the api Secret.

Users who close the laptop on a YOLO agent should be choosing that knowingly. The docs make sure they are.

---

## 9. v1 Ship Criteria

v1 is done when a user can, from their phone, with their laptop at home:

1. Create a task with a goal and watch a plan appear
2. Leave notes, get a revision, approve the plan
3. Receive a notification when the build reports done (or goes quiet, or blocks)
4. Review the full diff, request changes, get corrections
5. Merge — and at any point in 1–5, open the live terminal instead
6. Run two tasks in parallel on the same repo without them touching each other
7. Do all of the above with at least two different harnesses (Claude Code + one of Codex/OpenCode)
8. Verify that the relay stored nothing but ciphertext, and that Cloudflare carried nothing but DTLS ciphertext

Plus: templates overridable per project, credential class displayed correctly, and the security limitations documented where users will actually see them.

---

## 10. Open Questions

- **Plan-gate weight for small tasks.** "Fix this typo" doesn't deserve a plan document. Likely answer: a "quick task" dispatch that skips the plan phase (goal → build → review). Decide before the lifecycle UI hardens.
- **Quiescence threshold.** N seconds of PTY silence — fixed, or per-harness tuned? Start fixed (e.g., 90s), instrument, revisit.
- **Merge target semantics.** Merge to base locally vs. push branch + open PR (via `gh`). Probably a per-project setting; pick the default.
- **Warm-session cap.** What's the default concurrent-session limit, and is LRU-release-with-notification acceptable UX?
