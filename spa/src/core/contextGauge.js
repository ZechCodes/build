// How full the agent's context is: the small figure beside the paperclip.
//
// The tokens are the digest's `last_context_tokens` (wire 1.10) — what the last
// turn the agent's session reported held, null before one and again once a
// compaction is asked for. What they are a share of is where the chat compacts:
// the digest's `compact_at_tokens`, the threshold in effect. A chat that never
// compacts (0) is measured against the model's window instead. The catalog
// names that where it can (`context_window` on a model); until the bridge
// carries one, the harness says it, and a harness with no known window shows no
// figure rather than a guessed one.
//
// At rest the figure is the token total ("52k"); hover or focus expands it
// leftward to "52k/200k 26%", the total against its measure and the share. The
// node's text is only the total: the measure and the percent ride on `data-`
// attributes the stylesheet appends after it, so the expansion writes nothing.
//
// The figure is written onto a node of its own, never through the composer's
// markup: a push of the agent record lands while somebody is typing, and a
// repaint of the box would take the caret with it.

import { setAttr, setData, setHidden } from "../dom.js";
import { composerPartIds } from "./composer.js";

/** The windows the harnesses run at, by provider id. The Claude models all
 *  run with a 1M window, headless and TUI alike. */
const PROVIDER_WINDOWS = { claude: 1000000, claude_adk: 1000000 };

/** Where the steps change: dim below the first, the accent up to and including
 *  the second, the warning colour past it — where compaction is near and every
 *  turn is expensive. */
const ACCENT_FROM = 50;
const WARNING_ABOVE = 80;

const catalogModelOf = (agent, catalog) => {
  const modelId = agent.active_model || agent.choice?.model;
  if (!modelId) return null;
  const provider = (catalog?.providers || []).find((entry) => entry.id === agent.provider);
  return (provider?.models || []).find((model) => model.id === modelId) || null;
};

/** The window the agent's tokens are a share of, or null when nothing says. */
export function contextWindowOf(agent, catalog) {
  const named = catalogModelOf(agent, catalog)?.context_window;
  if (Number.isFinite(named) && named > 0) return named;
  return PROVIDER_WINDOWS[agent.provider] ?? null;
}

/** Tokens the way a person reads them: "612k", "1M", "1.5M". */
const tokenWord = (tokens) =>
  tokens >= 1000000 ? `${Number((tokens / 1000000).toFixed(1))}M` : `${Math.round(tokens / 1000)}k`;

/** What the tokens are measured against, and how the title says it: the
 *  compaction threshold while the chat compacts, the window when it never
 *  does. Null when there is neither. */
function measureOf(agent, catalog) {
  const threshold = agent.compact_at_tokens;
  if (Number.isFinite(threshold) && threshold > 0) {
    const word = tokenWord(threshold);
    return { tokens: threshold, words: `${word} tokens (compacts at ${word})` };
  }
  const window = contextWindowOf(agent, catalog);
  return window ? { tokens: window, words: `${tokenWord(window)} window` } : null;
}

function stepOf(percent) {
  if (percent > WARNING_ABOVE) return "warning";
  return percent >= ACCENT_FROM ? "accent" : "dim";
}

/** What the gauge says for one agent record, or null when it has nothing to
 *  say: no record, no turn yet (or a bridge from before the field), or
 *  nothing to measure against. */
export function contextGauge(agent, catalog) {
  const tokens = agent?.last_context_tokens;
  if (!Number.isFinite(tokens)) return null;
  const measure = measureOf(agent, catalog);
  if (!measure) return null;
  const percent = Math.round((tokens / measure.tokens) * 100);
  const text = tokenWord(tokens);
  const measureWord = tokenWord(measure.tokens);
  return {
    percent,
    text,
    measure: measureWord,
    expanded: `${text}/${measureWord} ${percent}%`,
    step: stepOf(percent),
    title: `${text} of ${measure.words}, ${percent}% used, as of its last turn`,
  };
}

/// Wire the gauge's own node. Returns a controller whose `set(agent, catalog)`
/// writes the figure, or hides the node when there is none; a call that would
/// say the same thing writes nothing. Null on a composer without the slot.
export function mountContextGauge(root, { ids }) {
  const node = root.querySelector(`#${composerPartIds(ids.input).gauge}`);
  if (!node) return null;
  let painted = null;
  return {
    set(agent, catalog) {
      const gauge = contextGauge(agent, catalog);
      const key = gauge ? `${gauge.expanded}|${gauge.step}|${gauge.title}` : "";
      if (key === painted) return;
      painted = key;
      setHidden(node, !gauge);
      if (!gauge) return;
      if (node.textContent !== gauge.text) node.textContent = gauge.text;
      setData(node, "measure", gauge.measure);
      setData(node, "percent", `${gauge.percent}%`);
      setData(node, "step", gauge.step);
      setAttr(node, "title", gauge.title);
      setAttr(node, "aria-label", gauge.title);
    },
  };
}
