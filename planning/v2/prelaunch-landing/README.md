# Handoff: Build prelaunch landing page (desktop + mobile)

## Overview
Pre-launch waitlist page for Build (getbuild.ing), the agentic coding IDE for teams. Goal: collect emails for the private beta. Two layouts — desktop (≥1024px) and mobile (≤480px) — share copy, palette, type and the animated dot-field hero; they differ in layout, nav, and the features section (bento grid vs. horizontal snap rail).

## About the design files
The HTML files in this bundle are **design references** — prototypes that show intended look and behavior. They are not production code to paste in. Recreate them in the live site's existing stack (`ZechCodes/build-web` templates + static assets) using its patterns. `dotfield.js` is framework-free vanilla JS and can be dropped in as-is.

## Fidelity
**High-fidelity.** Colors, type, spacing, copy and motion are final. Match pixel-for-pixel; text is verbatim.

## Files
- `Prelaunch Landing.html` — desktop reference
- `Prelaunch Landing Mobile.html` — mobile reference
- `dotfield.js` — hero dot-field animation (shared; used as-is by both)
- `assets/build-ide-screenshot.png` — product screenshot
- `Prelaunch Side by Side.html` — review harness only, not part of the site

## Design tokens
Colors
- `--g` accent green `#00ff88`; hover-link green `#7dffc0`
- `--bg` `#030604`; screenshot frame bg `#020503`
- `--bd` green hairline `rgba(0,255,136,.16)`; `--bd2` neutral hairline `rgba(255,255,255,.08)`
- `--t1` text `#f2fff8`; `--t2` secondary `#8fa89a`; `--t3` muted `#54655c`
- Button text on green `#02130a`
- Glows: text `0 0 28px rgba(0,255,136,.45)` (24px on mobile); button `0 0 32px rgba(0,255,136,.22)`, hover `0 0 44px rgba(0,255,136,.4)`

Typography — everything is `'JetBrains Mono', monospace` (Google Fonts, weights 400/500/600/700). `-webkit-font-smoothing:antialiased`.
- Desktop: h1 700 76px/1.04, ls -0.02em, uppercase · h2 (features) 700 36px/1.2 · h2 (CTA) 700 48px/1.1 · sub 400 16px/1.85 · card h3 600 18px uppercase ls .02em · card p 400 14.5px/1.8 · eyebrow/labels 500 12px ls .12em · nav links 500 13px · buttons 600 14px · footer 500 12px ls .08em
- Mobile: h1 700 34px/1.08 · h2 (features) 700 22px/1.24 · h2 (CTA) 700 28px/1.12 · sub 400 13.5px/1.75 · card h3 600 14.5px/1.35 · card p 400 13px/1.75 · eyebrow 500 10.5px ls .1em · buttons 600 13px · footer 500 10.5px ls .07em

Layout
- Desktop content width 1240px, 40px side padding. Mobile 20px side padding.
- Radius: **0 everywhere** (square corners are part of the look). Borders 1px.
- Motion easing `cubic-bezier(.16,1,.3,1)`; nav transition .3s.

Global overlay: `body::after` fixed, `repeating-linear-gradient(0deg, rgba(0,0,0,.15) 0 1px, transparent 1px 3px)`, opacity .45, pointer-events none, z-index 50 (scanline texture).

## Screens

### 1. Nav (both)
- `position:fixed; top:0`, height 60px desktop / 52px mobile. **Transparent over the hero**; once the hero scrolls out (IntersectionObserver on `.hero`, rootMargin `-{navHeight}px 0 0 0`) add `.solid`: `background rgba(3,6,4,.82)` (mobile .86), `backdrop-filter blur(14px)`, `border-bottom 1px var(--bd2)`. Transition background/border .3s.
- Left: brand — 18px (16px mobile) pentagonal-cube line SVG in green + `build_` 600 15px (14px) ls .04em.
- Desktop center: links `[features] [agents] [docs]` — 500 13px, color t2, padding 7px 13px, transparent 1px border; hover color green + border `--bd`.
- Right: `REQUEST ACCESS` solid green button, 600 13px, padding 9px 18px (9px 14px mobile), text `#02130a`; hover glow `0 0 28px rgba(0,255,136,.35)`. Links to `#waitlist`.
- Mobile adds a 44×44 hamburger (3 × 17×1.5px bars, t2) right of the CTA. Tap toggles a full-width dropdown under the nav (`rgba(3,6,4,.96)` + blur 14px, top/bottom hairlines); items 500 13px t2, padding 16px 20px, hairline separators. Open state: bars become a green X (translateY ±5.5px, rotate ±45°, middle bar opacity 0). Nav is also solid while the menu is open. Tap an item closes it.

### 2. Hero (both)
- `min-height:100vh`, flex, content vertically centered, **text centered**, `overflow:hidden`, bottom hairline `--bd2`. Desktop padding 110px top/bottom; mobile 56px top / 48px bottom.
- Background: `.field` absolutely fills the hero (z 0, pointer-events none) containing `<canvas id="cvL" data-gap="30">` (mobile `data-gap="24"`). Driven by `dotfield.js` — see Interactions.
- Content (z 1), top to bottom:
  1. Eyebrow `PRIVATE BETA — INVITES GOING OUT WEEKLY` — green, 500 12px ls .12em, margin-bottom 30px. Mobile: two lines (`PRIVATE BETA — INVITES` / `GOING OUT WEEKLY`), 10.5px ls .1em, mb 18px, line-height 1.6.
  2. h1 `Ship more.` / `Babysit less.` (second line in green with text glow).
  3. Sub (t2, max-width 620px, centered, margin-top 28px / 18px): "Build is the agentic coding IDE for teams. It surfaces the work that needs you — reviews, decisions, direction — and dispatches everything else to your agents. Not public yet."
  4. Waitlist form (see Components), margin-top 40px / 26px.
  5. Note (t3, 400 12px/1.7 ls .04em; mobile 11px ls .03em, mt 12px): "One email when your invite is ready. Nothing else."

### 3. Screenshot section (both)
- Desktop: padding 76px 0; radial glow behind `radial-gradient(ellipse 55% 45% at 50% 55%, rgba(0,255,136,.05), transparent 70%)`. Image inside `.winframe` (bg `#020503`), full width of the 1240 container.
- Mobile: padding 34px 0, glow `ellipse 70% 45% at 50% 50%` alpha .06; image full-bleed width.
- Asset: `assets/build-ide-screenshot.png`, alt "The Build IDE — commit history, diff review, and a live agent conversation".

### 4. Agents strip (both)
- Top + bottom hairline. Desktop: grid `220px repeat(4,1fr)`; first cell is the label `RUNS YOUR AGENTS` (t3, 500 12px ls .12em, no left border); each agent cell padding 26px 24px, left hairline (last also right), name 600 14px uppercase ls .04em t1, with `supported` below (400 12px t3, mt 5px, no transform).
- Mobile: label as a row (padding 16px 20px 0), then 2×2 grid; cells 16px 20px, 600 12.5px, top hairline, even cells get a left hairline.
- Agents: Claude Code, Codex, Pi, OpenCode.

### 5. Features (desktop — bento)
- Section padding 110px 0, bottom hairline. Header row: flex, baseline, space-between, mb 44px. h2 `Everything agents need.` / `Nothing you don't.` (`you don't.` in green). Right: `FEATURES /05` label (t3 500 12px ls .12em).
- 6-col grid, gap 12px. Card spans: 01 → 3, 02 → 3, 03 → 2, 04 → 4, 05 → 6.
- Card: bg `rgba(255,255,255,.02)`, 1px `--bd2`, padding 32px 30px 34px. Hover: border `--bd`, bg `rgba(0,255,136,.025)`, .2s.
- Card header row: `/0N` (green) left, category (t3) right — 500 12px ls .12em, mb 20px. Then h3, then p.

Card copy (verbatim):
- /01 TASKS — **An inbox, not a backlog** — Tasks live next to the code. Agents pick them up on their own — your inbox shows only the work waiting on a human. When it's empty, you're done.
- /02 VERSION CONTROL — **Worktrees that manage themselves** — Every agent works in its own worktree. Fetch, commit, review, merge — without leaving the conversation.
- /03 REVIEW — **Diff-first review** — Inline comments on any line, threaded with your team and your agents. Approve, redirect, or take over.
- /04 WORKFLOWS · RLM — **Workflows that reshape around the work** — The RLM routes tasks, retries failures, and re-plans as the work changes — escalating only when it genuinely needs a human. Your team sees the same board, live.
- /05 AGENTS — **Any agent, side by side** — Claude Code, Codex, Pi, and OpenCode run in parallel sessions — one surface, one shared history, one place to look.

### 6. Features (mobile — horizontal rail)
- Section padding 44px 0. Header stacked: h2 then `FEATURES /05` label (mt 12px).
- `.rail`: flex, gap 14px, `overflow-x:auto`, `scroll-snap-type:x mandatory`, hidden scrollbar, padding 18px 20px 22px 0 with the left edge bleeding to the viewport edge (margin-left -20px, padding-left 20px), trailing 6px spacer so the last card can center.
- Card: `flex:0 0 78vw; max-width:300px; scroll-snap-align:center`, bg `rgba(3,6,4,.88)`, 1px `--bd2`, padding 22px 18px 26px, `overflow:hidden`. Header 10.5px ls .1em mb 14px; h3 mb 9px.
- Centered card gets `.on`: border `--bd`, shadow `0 0 40px rgba(0,255,136,.12), inset 0 0 0 1px rgba(0,255,136,.06)`.
- Below the rail: 5 progress dashes (14×2px, `--bd2`, active = green, gap 6px, mt 6px).

### 7. Get in early CTA (both)
- `min-height:100vh`, flex column, vertically centered, text centered. Desktop padding 130px 40px; mobile 56px 20px. `id="waitlist"`.
- h2 `Get in early.` (`early.` green with glow). Note (t2, 400 13px ls .04em, mt 20px; mobile 10.5px/1.7 ls .06em mt 14px): `PRIVATE BETA · LOCAL-FIRST · E2E ENCRYPTED`. Waitlist form centered, mt 40px (24px mobile).

### 8. Footer (both)
- Top hairline. Desktop: flex space-between, padding 15px 40px. Mobile: stacked, gap 6px, padding 16px 20px. Text t3: `LOCAL-FIRST // E2E ENCRYPTED` and `© 2026 BUILD · GETBUILD.ING`.

## Components

### Waitlist form (`.wait`)
- Desktop: flex row, gap 10px, max-width 520px, wraps under 240px input min. Input flex 1: bg `rgba(255,255,255,.03)`, 1px `--bd2`, t1, 400 14px, padding 14px 16px, placeholder `you@company.com` in t3, focus border `--bd`, no outline. Button: green bg, `#02130a` text, 600 14px, padding 14px 26px, glow shadow, hover stronger glow. Label `REQUEST ACCESS`.
- Mobile: column, gap 10px, input full width padding 15px 14px 14px font; button full width, padding 16px 20px, 600 13px ls .04em.
- Submit: prevent default; if the email is invalid do nothing (browser validity, `novalidate` on form); on success replace the form with `✓ YOU’RE ON THE LIST — {email}` in green (500 14px ls .04em, mt 40px; mobile 13px/1.6 mt 26px). Wire the real backend here.

### Brand mark
Inline SVG, 24 viewBox, stroke `#00ff88` 1.6, round caps/joins, `fill:none`: `M12 2l8.5 5v10L12 22l-8.5-5V7L12 2z` and `M3.5 7L12 12l8.5-5M12 12v10`.

## Interactions & motion

### Hero dot field (`dotfield.js`, use as-is)
- Canvas fills the hero, DPR-aware (cap 2), resizes with the hero. Dots on a square grid (`data-gap` px), radius 1.1, base color `rgba(0,255,136,.07)`.
- Radial vignette: alpha scaled by distance from a point at (50%, 48%) — factor `clamp(.25, d²·1.6, 1)` — so dots are faint behind the copy and full strength at the edges.
- Ripples: every 1.5–3.3s one spawns at a random grid point, radius grows .09px/ms (30% chance of a "big" one: .13px/ms, 110px wide band vs 70px), fades out over ~0.9× the larger canvas dimension. Dots within the band brighten (alpha up to ~.82, radius up to 2.7px) with a quadratic falloff; strongly excited dots get an 8px green glow.
- NEEDS YOU flag: 1.8s after load, then every 5.5–8.5s, one dot away from the center (outside .75 of the vignette ellipse, ≥20px from edges, ≥110px from the right) is flagged for 5.2s: white 2.6px dot, four 10px white corner brackets blinking 650ms on / 250ms off, and the mono label `NEEDS YOU` (500 10px, white `#eafff4`) to its right. When the flag expires a big ripple spawns from that dot (the work got done).

### Nav
Fixed, transparent over hero, `.solid` after hero leaves the viewport (see Nav). Mobile menu toggle as described.

### Mobile feature rail motion
- Native scroll + snap. A rAF loop runs only while scrolling: velocity = Δscroll/Δt (×16, per-frame), smoothed with `sv += (vel - sv) * .08`, normalized `v = clamp(sv/60, -1, 1)`.
- Every card: `transform: perspective(900px) rotateY(-v·16deg) scale(1 - |v|·.03)`; border color `rgba(0,255,136, .08 + |v|·.3)` while moving; a light sweep pseudo-element (`linear-gradient(105deg, transparent 30%, rgba(0,255,136,.14) 50%, transparent 70%)`) translated `v·120%` with opacity `|v|·.9`.
- Cards have `transition: transform .5s cubic-bezier(.16,1,.3,1), border-color .4s, box-shadow .4s`. When velocity dies (|vel|<.02 and |sv|<.2) transforms/border are cleared and the cards spring flat.
- The card whose center is nearest the rail center gets `.on`; the matching dash turns green.

### Desktop
Card hover (border/bg .2s). Button hovers (glow). Link hover color bump. No other motion.

## State
- `menuOpen` (mobile)
- `navSolid` (both)
- per-form `submitted` + email value
- rail: `activeIndex` (0–4), transient scroll velocity

## Assets
- `assets/build-ide-screenshot.png` (product screenshot, provided by Build)
- Brand mark: inline SVG above
- Font: JetBrains Mono via Google Fonts `family=JetBrains+Mono:wght@400;500;600;700`

## Responsive
Only the two breakpoints above were designed. Suggested switch: mobile layout ≤ 640px, desktop ≥ 1024px; between them use the desktop layout with the bento collapsing to 3-col spans (cards 03/04 → full width) and h1 scaling `clamp(44px, 6vw, 76px)`.
