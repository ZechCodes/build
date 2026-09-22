# Build landing page: story and direction

Issue #75. Planning only: nothing on the page changes until Zech approves this document.
Written by the Fable lead, converged with the Astra co-planner on the issue.

**Working title:** *Your agents. Your machine. Your call.*

**Through-line:** everything on this page happens on a computer you own. Every act either
shows work running on your machine or shows you deciding what leaves it. The laptop is the
machine and never stops being the anchor; the phone and tablet are windows into it, never a
second place where code lives. "Control" on this page means four concrete things, and each
act proves one of them: agents execute on your hardware, they change your checkouts, you can
change any line and any workflow yourself, and you decide what ships.

**Honesty rule.** The page sells the final product. Every screen is an authored
demonstration state of that product, drawn in the current Build visual language. The
practical section under the story (hosts, harnesses, storage) keeps describing what exists
today, so the film can be ambitious without the page lying about the alpha.

**Call to action (decided by Zech on #75).** The primary button everywhere is **Join the
waitlist**. Access is invite-only until the alpha reaches beta-level stability, so there
is no Download button on the page. The waitlist endpoint and form already exist
(`POST /api/waitlist`, `landing/waitlist.html`). The secondary link is GitHub.

---

## 1. The story

One continuous example carries the whole page: **make archived items searchable**. The
visitor sees the same file, the same issue, the same branch, and the same test from the
first screen to the merge. That is what makes six features read as one product instead of
a list.

| # | Act | Headline | What it says | What the visitor feels |
|---|---|---|---|---|
| 1 | **Open** | Your agents. Your machine. Your call. | Build is an IDE for agentic coding. The lid opens on a real editor, and a human types a line into it. | Recognition, then ownership. "That's an IDE, and it's mine." |
| 2 | **Where it runs** | The work runs on your machine. | Agents execute on your hardware and change your checkouts. The hosted service does sign-in and the handshake, nothing more. | Relief. Trust. |
| 3 | **Hand it off** | Say what needs doing. | Write the issue where the code lives. Assign it. An agent cuts a branch on your machine and starts. | Leverage. |
| 4 | **The day moves** | One issue. A whole team. | Agents fan out into their own worktrees. One asks a question. You answer from your phone. The laptop keeps working. | Momentum. |
| 5 | **Wire it** | Build the workflow. Then run it again. | The path you just watched is a workflow. Open the builder, add a test step, move the review, put the gate where you want your call. | Authorship. Power, and a little play. |
| 6 | **Everything has a home** | Every change lands in Git. | Your line and the agents' lines on one branch. Diff, stage, commit, without leaving the window. | Legibility. Nothing hidden. |
| 7 | **Your call** | See what needs you. Decide what ships. | Agents triage the changes. What needs your judgment comes first, with the diff and the evidence beside it. You approve. Build merges. The climax. | Control. Confidence. |
| 8 | **Any screen** | Your work stays put. You don't have to. → Your machine. Your call. | The lineup at true physical scale. Then the call to action. | Resolution. Invitation. |

**Why this order.** The page follows the shape of a piece of work rather than a feature
list: where you sit (editor) → where it runs (the machine) → what to do (issue) → who does
it (agents) → how it's composed (workflow) → where it lands (Git) → your decision (review)
→ where you can be while all that happens (any screen). Each act is a consequence of the
one before it, and the page builds toward review, which is the product's real promise: you
keep the final say, and you only get asked when it matters.

**The human is the author at both ends.** In act 1 the visitor watches a person type one
test line, `keeps archived items in search`. In act 7 the evidence beside the diff is that
test passing. Everything between is agents making a human's intent true, on the human's
machine, under the human's gates. That is the control story told as a plot instead of a
claim.

**What changes from today's page.** Today's arc (Start → Handoff → Direction → Overview →
Review → Download) sells reach across screens: a laptop, then a phone, then a phone again,
then a tablet. A visitor can reasonably read it as an agent monitoring dashboard. The new
arc keeps its best beats (the lid opening, the phone answering a question, the tablet
handing off to a live HTML review surface, the closing lineup) and puts them under the
control claim, with the editor, Git, and the workflow builder proving Build is an IDE
before any remote screen appears.

---

## 2. Storyboard

Conventions. Scroll lengths are in viewport heights (vh) of pinned travel; the whole
story is about 850vh, then the page returns to normal document flow for the practical
section. "Local" is progress within an act, 0 to 1. Device poses use the existing
manifest vocabulary (x, y, width as percent of stage, yaw/pitch/roll, opacity, lidOpen,
faceCamera). Copy is real draft copy in the brand voice: second person, short sentences,
one benefit per sentence, no buzzwords, no exclamation points. Screen textures named
`ui1x-*` are new; `ui0x-*` already exist in `skriftapp/buildapp/landing/assets/screens/`.

Desktop and tablet viewports run the pinned film. Phones and reduced-motion readers get the
document version: the same acts in order, each as a still plus copy plus one legible HTML
close-up. Nothing depends on the 3D to be understood (see section 5).

### Act 1. Open (about 100vh)

- **Kicker:** 01 / Your machine
- **Headline (h1):** Your agents. Your machine. Your call.
- **Support:** Build is the IDE for agentic coding. Agents run on your hardware, in your
  checkouts. You decide what ships.
- **Meta line:** Free and open source · Invite-only alpha
- **Actions:** primary "Join the waitlist" (opens the waitlist form in act 8 or an inline
  email field), secondary "See how it works" (scrolls to act 2).
- **Devices:** laptop only, low front view with 4° of pitch (the existing hero pose,
  x 65, y 57, w 52). Copy on the left.
- **Screen:** `ui10-editor-macbook` (new). A real editor: file tree on the left
  (`build-app/src/search/archive.ts`, `archive.test.ts`), code in the middle, a collapsed
  terminal drawer along the bottom, agents rail on the right with "Fixing archive button ·
  Claude Code · Working". Status bar carries the machine label: `zech-mbp · ~/code/build ·
  main`. That label persists on every laptop screen for the rest of the page.
- **Beats:**
  - local 0 → 0.40: the lid opens from closed to 105° on the real hinge while the base
    stays planted (existing behaviour). Copy and CTA are readable before the lid moves.
  - 0.40 → 0.55: hold on the open editor.
  - 0.55 → 0.75: the human action. A caret appears in `archive.test.ts` and a real test
    types in, character by character, driven by scroll: the title line
    `it("keeps archived items in search", () => {` and then the expectation
    `expect(search("launch notes")).toContain(archived)`. The gutter marks both lines as
    changed; the status bar ticks to `2 changes · you`. This is an HTML overlay aligned to
    the screen plane (the existing review alignment technique), not a texture swap, so it
    can type. The point is a meaningful edit, not a title: the visitor sees the human state
    what "done" means before any agent touches the code.
  - 0.75 → 1.0: copy fades; the laptop eases back and down toward center for act 2.
- **Document version:** still of the open laptop with the editor, plus a before/after
  pair of HTML editor cards under the copy (the file without the test, then with it).

### Act 2. Where it runs (about 60vh)

- **Kicker:** 02 / Where it runs
- **Headline:** The work runs on your machine.
- **Support:** Your agents run where your code lives. Connect from another screen and stay
  in control of the same work.
- **Three captions,** revealed in sequence beside the laptop, each one line:
  - Agents execute here.
  - Checkouts change here.
  - Build's service handles sign-in and pairing. It never holds your code.
- **Devices:** laptop only, centered and smaller (w 40), turned 12° toward the copy.
  No phone, no tablet. The machine label on the status bar is legible.
- **Screen:** `ui10-editor-macbook` (same as act 1, continuity).
- **Beats:** laptop settles (0 → 0.25); captions fade up one at a time (0.25, 0.45, 0.65)
  and hold; exit (0.85 → 1.0). Nothing else moves. This act is the claim, not a feature,
  and it is short on purpose. If the animatic feels slow, fold the three captions into the
  hold of act 1 and drop the act.
- **Document version:** the three captions as a list under the copy, no image.

### Act 3. Hand it off (about 100vh)

- **Kicker:** 03 / Issues
- **Headline:** Say what needs doing.
- **Support:** Write the issue where the code lives. Assign it to an agent. It cuts a branch
  on your machine and starts. You write the next one.
- **Devices:** laptop, three-quarter view from the left (x 68, y 58, w 50, yaw −12), opaque.
- **Screen:** `ui12-issues-macbook` (new). Build's issue board: columns Backlog, Ready, In
  progress, In review. One card in Ready: "Make archived items searchable", labels `search`,
  `bug`. The machine label stays in the status bar.
- **HTML close-up:** the issue card, aligned to the screen plane then growing out of it
  into a readable card (same technique as the current review surface).
- **Beats:**
  - 0 → 0.20: laptop arrives; issue board visible.
  - 0.20 → 0.35: the card lifts out of the screen into the HTML close-up.
  - 0.35 → 0.50: the assignee chip fills: "Claude Code".
  - 0.50 → 0.70: the card slides from Ready to In progress; under it, a branch line appears:
    `build/archive-search · ~/code/build/.worktrees/archive-search`.
  - 0.70 → 0.85: hold. 0.85 → 1.0: the card returns to the screen; exit.
- **Document version:** still plus the issue card in its In progress state.

### Act 4. The day moves (about 110vh)

- **Kicker:** 04 / Orchestration
- **Headline:** One issue. A whole team.
- **Support:** The agent splits the work: one implements, one reviews, one audits the edge
  cases. Each runs in its own worktree on your machine. When one needs you, it asks. You
  answer from the screen in your hand.
- **Devices:** laptop stays open on the left as the host (x 30, y 64, w 34, yaw 12, opacity
  1, not the current 0.58 fade). Phone pivots in on the right (existing entrance offset:
  from x+13, y+10, scale 0.72, rotated 28°/−8°/8°, settling to x 83, y 56, w 21).
- **Screens:**
  - laptop: `ui13-team-macbook` (new, derived from the existing `ui04` activity layout):
    three agent rows, each with a worktree path and a harness name; the terminal drawer
    open one line to show a test run scrolling. Machine label present.
  - phone: existing `ui03-question-iphone` → `ui03-answer-iphone` → `ui03-resumed-iphone`
    ("Keep archived items in search?" / "Yes. Label them clearly." / "Got it. I'll update
    the archive behavior and checks.").
- **Beats:**
  - 0 → 0.30: fan-out on the laptop. One agent row becomes three, each row sliding in with
    its worktree label. A small "3 running" counter increments.
  - 0.30 → 0.45: the phone pivots in and settles. Its screen already shows the question
    (texture loaded before the pivot, as the runtime does today).
  - 0.45 → 0.60: the question is read (hold). 0.60: the answer texture swaps in.
    0.75: the resumed texture swaps in; on the laptop, the implementing agent's row goes
    from "Waiting" to "Working".
  - 0.85 → 1.0: the phone departs to the right; the laptop stays.
- **Document version:** two stills (the fanned-out laptop, the phone at "answer") with the
  three-line conversation as an HTML card.

### Act 5. Wire it (about 120vh)

- **Kicker:** 05 / Workflows
- **Headline:** Build the workflow. Then run it again.
- **Support:** The path you just watched is a workflow: implement, review, decide. Open it
  in the builder. Add a test step. Move the review. Put the gate where you want your call.
  Run it on the next issue.
- **Devices:** laptop, front and center, large (x 50, y 60, w 66, yaw 0), aligned to the
  layout plane (faceCamera 0) so the screen hands off to HTML cleanly.
- **Screen:** `ui16-builder-macbook` (new). The interactive builder: a canvas with four
  nodes on one path, Implement → Review → You decide → Merge, each node carrying the agent
  that runs it. A node palette on the left (Agent, Test, Review, Human gate, Fan out).
- **HTML close-up:** the builder canvas, aligned to the screen plane and then taken over by
  HTML so nodes and edges can move.
- **Beats (this act must change a real handoff, not decorate a graph):**
  - 0 → 0.15: laptop settles; the canvas is visible with the four-node path. The Implement
    node is marked "Done · handoff pending": the implementing agent from act 4 has just
    finished and its handoff to Review has not happened yet.
  - 0.15 → 0.20: match cut to the HTML canvas.
  - 0.20 → 0.35: hold; the visitor reads the path as the one from act 4.
  - 0.35 → 0.55: a **Test** node is dragged from the palette and dropped between Implement
    and Review. The Implement → Review edge visibly detaches, and two new edges draw:
    Implement → Test → Review. Node labels: "Test · runs archive.test.ts".
  - 0.55 → 0.70: the pending handoff runs on the new path: one pulse leaves the finished
    Implement node, Test lights green, Review runs, and the pulse stops at **You decide**,
    which pulses and waits. The change governs the handoff that is about to happen; it
    does not rewrite work already done, and nothing upstream re-runs.
  - 0.70 → 0.85: hold on the waiting gate. A small caption under the canvas: "Saved as
    *Fix with tests*. Runs on the next issue."
  - 0.85 → 1.0: the canvas returns to the screen; the laptop pulls back for act 6.
- **Document version:** two stills of the HTML canvas (before and after the test node),
  side by side.

### Act 6. Everything has a home (about 100vh)

- **Kicker:** 06 / Git
- **Headline:** Every change lands in Git.
- **Support:** Your line and the agents' lines, on one branch, in one window. Read the
  diff. Stage it. Commit it. The editor and the Git tools are the same app.
- **Devices:** laptop, near-frontal close-up (x 50, y 58, w 60), aligned for HTML handoff.
- **Screen:** `ui14-git-macbook` (new). The editor with the Git panel open: branch
  `build/archive-search`, worktree path, a small commit graph (main, the branch, three
  agent commits), and the working-tree list with `archive.ts` and `archive.test.ts`
  modified. The diff for `archive.ts` is in the editor: `search.remove(id)` removed,
  `search.update(id, { archived: true })` added (the same lines the review shows today).
- **HTML close-up:** the diff and the Git panel.
- **Beats:**
  - 0 → 0.20: laptop arrives; match cut to HTML.
  - 0.20 → 0.40: the diff gutter highlights the agent's change (+2 −1) and then the
    visitor's test line from act 1 in `archive.test.ts`, with author chips: "Claude Code",
    "you".
  - 0.40 → 0.55: the human edits one more line directly in the diff: the label string
    becomes `"Archived"`. The gutter updates live.
  - 0.55 → 0.75: Stage all; the commit message types in: `archive: keep archived results
    searchable`; Commit. The graph gains one commit on the branch with a "you" chip.
  - 0.75 → 0.85: hold. 0.85 → 1.0: return to screen; the laptop pulls back and left
    into its act 7 host pose (x 22, y 66, w 26, yaw 14), the Git panel still showing the
    new commit. It does not leave.
- **Document version:** still of the Git panel plus the diff as an HTML code block.

### Act 7. Your call (about 140vh, the longest act)

- **Kicker:** 07 / Review
- **Headline:** See what needs you. Decide what ships.
- **Support:** Agents triage every change before you see it. What needs your judgment
  comes first, with the diff and the evidence beside it. Everything else stays one tap away.
  Read. Decide. Build merges.
- **Devices:** the laptop stays open on the left as the host (the act 6 exit pose, x 22,
  y 66, w 26, yaw 14, opacity 0.9) for the whole act, the same rule as act 4. The tablet,
  landscape, pivots in from the right (existing entrance) and settles to the exact HTML
  alignment the runtime already implements for the review surface (rotation zeroed,
  faceCamera 0, width matched to the HTML surface).
- **Screens:** tablet `ui15-triage-ipad` (new) → existing `ui05-approval-ipad` →
  existing `ui05-merged-ipad`. Reuse the existing iPad system UI treatment (status bar,
  window controls, Dock, home indicator). Laptop: `ui14-git-macbook` carried over from
  act 6, swapping to existing `ui05-merged-macbook` at the merged beat.
- **HTML review surface:** the triage list, then the finding open with summary, diff, and
  evidence, then approval, then merged. This is an evolution of today's `demo--review`.
- **Beats:**
  - 0 → 0.15: tablet pivots in, screen already showing the triage list.
  - 0.15 → 0.25: align and hand off to HTML (existing technique).
  - 0.25 → 0.40: **triage.** The list reads: "Needs you · 1: Search index behaviour
    changed", "Verified · 3", "Failed · 0", each expandable. The list narrows: the three
    verified rows compress to a single collapsed row that stays visible and tappable; the
    one finding rises to the top. Nothing disappears.
  - 0.40 → 0.65: the finding opens. Summary: "Keep archived items searchable and visibly
    labeled." Diff: the same lines from act 6. Evidence: `archive.test.ts passed`,
    "keeps archived items in search ✓" (the visitor's test from act 1), "Checks passed".
    This is stable reading time: at least half the act is a still surface.
  - 0.65 → 0.80: the approval control appears; a human approves ("Approved for merge.").
    Shown as a separate beat from the merge so the decision is visibly a person's.
  - 0.80 → 0.90: merged state (`ui05-merged-ipad`): "Merged · Archive behavior updated".
    The laptop screen swaps to `ui05-merged-macbook` in the same beat: the merge lands on
    the machine, and the tablet only shows it.
  - 0.90 → 1.0: the surface returns to the tablet; the tablet departs to the right. The
    laptop stays.
- **Document version:** three stills (triage, finding open, merged) with the triage list as
  an HTML card.

### Act 8. Any screen (about 90vh, two copy beats over one pose)

- **Kicker:** 08 / Any screen
- **Beat A headline:** Your work stays put. You don't have to.
- **Beat A support:** Check in, answer a question, review what's ready from your phone or
  tablet. The agents keep working on the machine you chose.
- **Beat B headline:** Your machine. Your call.
- **Beat B support:** Free and open source. Runs on the hardware you already own.
- **Actions:** the waitlist form itself (email field and "Join the waitlist" button, the
  existing `waitlist.html` form and client), secondary "GitHub". Meta line: "Invite-only
  alpha. Free and open source."
- **Devices:** the existing physical-scale lineup (laptop x 32 y 72 w 34; tablet x 58
  y 75.8 w 27.2; phone x 77.8 y 77.1 w 8.5), laptop dominant on the left.
- **Screens:** `ui05-merged-macbook`, `ui05-merged-ipad`, `ui05-merged-iphone` (all
  existing): the same finished work on every screen, not three unrelated demos.
- **Beats:** the laptop eases from its act 7 host pose into its lineup pose; tablet and
  phone arrive with the existing settle (0 → 0.35). Beat A copy holds (0.35 → 0.60).
  Beat A crossfades to Beat B and the CTAs (0.60 → 0.75). Hold. The pin ends and the page
  returns to document flow into the practical section.
- **Document version:** the lineup still, Beat B copy, and the waitlist form.

### Below the story: the practical section (document flow)

Kept from today with refreshed copy, still server-rendered from real data: "Your hosts do
the work" (how a host works, without the download chooser: the install command and host
builds move to `/docs` for invited users), the three FAQ items (what runs where, which
agents, what Build stores, linking to `/docs` and `/privacy`), the activity feed when it
has entries, and the footer. This is where the alpha state is stated plainly: invite-only
until beta-level stability, waitlist for everyone else.

### The document version (phones, reduced motion, no WebGL, no JavaScript)

The same eight sections in the same order, as ordinary document flow. No pinning, no
transforms, and no content hidden waiting for scroll or script. Each section has its copy,
an intrinsically sized device poster, and the readable HTML proof named in its "Document
version" line above: before/after cards for the test edit (act 1) and the workflow
reconnection (act 5), the question and answer text (act 4), and the triage finding with
its diff and evidence together (act 7). Buttons, links, and the waitlist form work
normally. The WebGL canvas is decorative and `aria-hidden`. One final-state poster alone
would erase the causal story, so every act keeps its own still. The existing behaviour for
slow renderers, lost contexts, `save-data`, and reduced motion (hand back to posters, keep
the document readable) is the baseline and carries over.

---

## 3. The feature story

Each feature is the next state of the same job, so the page builds instead of listing.

| Order | Feature | Act | Its moment | What it proves about control |
|---|---|---|---|---|
| 1 | **Code editor** | 1 (and 6) | The lid opens on an editor and a human types a test line into it. | It is your editor; you can type in it. |
| 2 | **Issue tracking** | 3 | The issue is written beside the code, assigned, and moves to In progress with a branch on your machine. | Work is described and assigned where the code lives. |
| 3 | **Agent orchestration** | 4 | One agent becomes three, each in its own worktree; one asks, you answer from a phone, it resumes. | Agents execute on your hardware, in your checkouts, and ask before they guess. |
| 4 | **Dynamic workflows + interactive builder** | 5 | The path from act 4 is opened as a graph; a test node is inserted; edges reconnect; a pulse stops at the human gate. | You author how work happens and where your call sits. |
| 5 | **Git tools** | 6 | Your line and the agents' lines on one branch; edit in the diff; stage; commit. | Nothing an agent did is invisible; you can change any of it. |
| 6 | **Review surface with agent triage** | 7 | Triage surfaces one finding, keeps the rest inspectable, shows the diff and the evidence (your test passing), you approve, Build merges. | You decide what ships. |

Editor before issues because the visitor has to know what kind of product this is before
any agent appears. Orchestration before workflows because the builder edits a path the
visitor has already watched, which is what makes the graph mean something. Git before
review because review needs the branch and the commit to exist. Review last because it is
the promise, and a page should end its argument on its strongest proof, then close.

---

## 4. Framework recommendation

### What the page is built with today

- Hand-written vanilla ES modules in `skriftapp/buildapp/landing/` (no bundler for the
  page itself): `cinematic-story.js` (scroll → frame state, pinning, profiles),
  `story-manifest.js` (six scenes, per-profile poses, checkpoints), `story-devices.js`
  (poster fallbacks), `device-stage.js` (three.js stage: GLTF loading, screen textures,
  hinge, solid fades, frame budget, poster handoff), `device-lighting.js`.
- three.js 0.180 vendored as one prebuilt bundle (`landing/vendor/three-device-runtime.js`,
  built by esbuild from `design/landing-runtime/`), with unit tests for the stage math.
- `index.html` is a **body fragment**. `root_controller.render_landing_page` wraps it in
  `landing/shell.html` with `fill_slots`, injecting the practical section (install command,
  platform download links from `releases`, the activity feed). `/landing/{path}` serves the
  directory's files with a fixed media-type map. `scripts/preview-landing.py` mounts the same
  `RootController` on a loopback Litestar app with the production CSP:
  `script-src 'self' 'wasm-unsafe-eval'`, `style-src 'self' 'unsafe-inline'`, no CDN.
- The SPA (`spa/`) is a Vite build emitted into `skriftapp/buildapp/static/` (gitignored)
  by a node stage in `skriftapp/Containerfile`. That is the pattern to copy.

### Recommendation: Astro (static) + GSAP ScrollTrigger + plain three.js

**Astro** for the page. It is the popular tool built for exactly this: a static, content-
first site with zero JavaScript by default and per-component islands where we need them.
It outputs a plain `dist/`, runs on Vite (same toolchain as the SPA), and needs no server
runtime. Next.js was considered: it can static-export, but it brings the React runtime for a
page whose only interactive parts are one WebGL canvas and scroll timelines, and this repo's
web client is deliberately framework-free.

**GSAP ScrollTrigger** for the film. Pinned sections with scrubbed timelines are the genre's
standard tool; it gives per-act timelines with labels (the "beats" above map one to one),
pin/unpin, `scrub` smoothing, and `matchMedia` for the desktop/tablet/document profiles,
and it does not require a framework. Framer Motion is React-bound and its scroll primitives
are weaker for multi-act pinning. GSAP is free for this use since 3.13. No smooth-scroll
library (Lenis and the like): native scroll plus scrub is enough and avoids hijacking.

**Plain three.js** for the devices, carried over from the current stage rather than
rewritten in React Three Fiber. The current stage already solves the hard parts and has
tests: unlit untone-mapped screen materials, the hinge, per-device depth passes so fading
hardware stays solid, poster fallback when WebGL is slow or missing, texture handoff that
never blinks to an empty screen. R3F would add React solely to drive three models. What
does get replaced is the **choreography**: `device-stage.js` hard-codes scene indices and
screen mappings around the old six chapters (`deviceNamesForScene`, `getDeviceScreenSource`,
the review-frame special case). The new build keeps the renderer, materials, loader, and
fade code, and drives poses and screen sources from GSAP timelines per act instead.

**HTML close-ups** (the typed test, the issue card, the builder canvas, the Git panel, the
review surface) are ordinary DOM animated by the same timelines, aligned to the device
screen plane using the alignment the review surface uses today. That is where most of the
"Apple" feel comes from, and it needs no 3D.

### How skriftapp serves it

A static build, dropped into the current serving path, with one integration change.

- Source lives in a new `landing/` project at the repo root (beside `spa/`), Astro +
  GSAP + three.js as npm dependencies, self-hosted fonts and the vendored runtime moved
  in. `npm run build` emits into `skriftapp/buildapp/landing/generated/` (gitignored,
  like `static/`), and the Containerfile's node stage builds it next to the SPA.
- **The shared landing directory stays.** `shell.html` and the fragments the other routes
  render (`panel.html`, `panel-button.html`, `panel-link.html`, `waitlist.html`,
  `unsubscribe-form.html`, `unsubscribe-address.html`, `activity-section.html`) are not
  homepage files and are untouched. Only the homepage's own files are retired when the
  generated page lands: `index.html`, `practical.html`, the story scripts
  (`cinematic-story.js`, `story-manifest.js`, `story-devices.js`, `device-stage.js`,
  `device-lighting.js`, `main.js`), `cinematic.css`, and the assets the generated build
  now carries itself.
- `/landing/{path}` keeps serving the directory unchanged, generated subdirectory
  included; the media-type map already covers `.js`, `.css`, `.webp`, `.glb`, `.woff2`,
  `.json`.
- **`/` serves the generated full document**, not a fragment. Astro emits a complete
  `<html>`, so `render_landing_page` must not wrap it in `shell.html` (that would nest
  shells). The generated page carries the same head, fonts, favicon, and social meta itself.
- **Dynamic content stays dynamic, and only what the homepage still shows.** The practical
  section needs the activity feed and the repository link at request time. The Astro page
  leaves the existing `{{activity_section}}` and `{{repository_url}}` markers in its HTML,
  and `render_landing_page` runs `fill_slots` over the generated document exactly as it
  does today. The `{{install_command}}` and `{{platforms}}` slots leave the homepage with
  the download chooser: install commands and host builds belong to `/docs` for invited
  users, and a waitlist page must not advertise a download it cannot offer. Nothing else
  is templated.
- Every other public route (`/docs`, `/privacy`, invite and unsubscribe pages, the
  installer scripts, `/app/`) keeps its current shell and code path.
- **CSP.** Bundles must be external same-origin files. Astro inlines small scripts by
  default; set `vite.build.assetsInlineLimit: 0` (or use Astro's CSP integration) so no
  inline `<script>` is emitted. GSAP writes inline `style` attributes, which the existing
  `style-src 'unsafe-inline'` allows. No CDN.
- **Waitlist.** `POST /api/waitlist` already exists (`waitlist_controller.py`, registered
  in `skriftapp/app.yaml`) and the landing already has the form and client
  (`waitlist.html`, `waitlist-form.js`, `waitlist-api.js`). The CTA needs no new signup
  infrastructure. Note that `preview-landing.py` registers only `RootController`, so the
  preview does not exercise submission; the implementation issue should either register
  the waitlist controller in the preview or document the form's preview-only state.
- `preview-landing.py` and the landing browser checks (`web/landing-check.mjs`,
  `web/landing-record.mjs`) keep working against the built output; `test_root_landing.py`
  and `test_landing_page.py` change from "fragment wrapped in shell" to "generated document
  with slots filled".

---

## 5. What the 3D assets can do now, and what needs new work

### As they are

- Three original GLB models at true scale with a shared contract (`device-contract.js`,
  `metadata.json`): 14" MacBook Pro (`laptop-low.glb`, 1.7 MB, with a working `laptop_lid`
  hinge from closed to 105°), 11" iPad Pro (196 KB), iPhone 17 Pro Max (700 KB). One
  replaceable `screen` mesh each, native display aspect, tested geometry.
- A perspective stage (20° camera, studio environment, key/fill/edge lights, soft shadows)
  that poses any device by stage-percent position, width, yaw/pitch/roll, opacity, and
  optional camera-facing compensation. Any pose in this storyboard is expressible today.
- The lid opening (any `lidOpen` value, so any partial angle too), phone and tablet pivot
  entrances that settle without idle motion, solid cross-fades between devices, a fixed
  layer order in multi-device frames, and the physical-scale lineup.
- Screen textures swapped per device per beat with the previous display kept until the
  next has loaded; system UI (menu bar, Dock, status bars, Dynamic Island, home indicator)
  baked into the existing captures via `design/landing-captures/`.
- The tablet-to-HTML alignment (screen plane matched to a DOM surface) that acts 1, 3, 5,
  6, and 7 all reuse. It is written for the tablet in scene 4; generalizing it to the
  laptop is a pose change, not new rendering (the laptop screen is tilted 15° from vertical
  at full open, so the laptop close-ups should align to the screen plane, not the base).
- Poster fallbacks: transparent device cutouts and six responsive scene posters rendered by
  the Blender generator, plus the frame-budget logic that hands back to posters on slow
  machines and the document mode under 768px, on `save-data`, or with reduced motion.
- Existing screen textures usable as-is: `ui01` (agents running), `ui02` (inbox),
  `ui03-question/answer/resumed` (the phone conversation), `ui04` (activity),
  `ui05-approval` and `ui05-merged` on all three devices.

### Needs new textures (captured through the existing fixture, `design/landing-captures/`)

Each new state is a new entry in `screen-manifest.json` plus fixture markup; the capture
script then emits native-resolution masters and the runtime WebPs with system UI. These are
authored demonstration states of the final product, so several need UI that the SPA does
not have yet (the fixture is allowed to draw it; it is marketing artwork, not the client).

| Texture | Device | Act | What it shows |
|---|---|---|---|
| `ui10-editor-macbook` | laptop | 1, 2 | Full editor: file tree, code, collapsed terminal drawer, agents rail, machine label. |
| `ui12-issues-macbook` | laptop | 3 | Issue board with the archived-search card in Ready. |
| `ui13-team-macbook` | laptop | 4 | Three agents with worktree paths, terminal drawer showing a test run. |
| `ui16-builder-macbook` | laptop | 5 | Workflow builder canvas with the four-node path and the palette. |
| `ui14-git-macbook` | laptop | 6 | Editor with Git panel: branch, worktree, commit graph, diff. |
| `ui15-triage-ipad` | tablet | 7 | Triage list: needs you / verified / failed. |
| `ui05-merged-*` | all | 8 | Existing; verify the merged state reads as the same job. |

Plus the machine label on every laptop screen, which today's textures lack.

### Needs new scenes or posters

- **Eight scene posters** (sixteen files: desktop and mobile pairs) instead of six.
  `render_scene_posters` in `design/landing/build_device_assets.py` authors a fixed six
  and aliases `scene-06` as the closing poster (`desktop-poster.webp`, `mobile-poster.webp`);
  it needs the new scene definitions and screen assignments, the closing alias moved to
  scene 08, a refreshed `social-preview.webp` and the `og:image` PNG, and the capture
  manifest and `metadata.json` (render bounds, hashes, provenance) kept in sync.
  Alternatively capture the posters from the live stage at 1440×900 and 720×960 as the
  review stills were, which is faster and matches the live look. Either way the reduced-
  motion and phone readers see the new final-product screens; the old six posters are not
  an adequate fallback because they tell the old story.
- **New choreography** (poses per act, screen source per beat, HTML alignment for the
  laptop): a new timeline, replacing the scene-index logic in `device-stage.js`.
- **Nothing requires new geometry, materials, or Blender work.** No rear views, no
  portrait tablet, no exploded views, no depth-of-field or bloom (the runtime bundle has
  no post-processing and should stay that way for the frame budget).

### Deliberately not doing

- No lid-close shot (implies a sleeping laptop keeps running).
- No code or execution "moving" to the phone or tablet; the laptop stays open on screen
  whenever a remote device is present.
- No glowing boundary or "nothing leaves this machine" imagery: remote screens receive
  diffs, and the harness's model provider handles what the agent sends it.

---

## 6. Open questions for Zech

The CTA question is settled ("Join the waitlist", invite-only until beta-level stability,
no Download on the page). Two remain.

1. **The control claim.** Is the promise "agents and checkouts run on hardware I choose,
   with the model providers I configure", or should the page also promise no external
   data transfer (local inference)? The storyboard assumes the first.
2. **Palette.** Keep Build's current look (black, mint accent, Inter and JetBrains Mono,
   as in the stills) for the new page, or move it toward the zech.sh navy-and-cyan brand?
   The storyboard assumes Build's current look.
