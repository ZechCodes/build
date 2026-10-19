// Context usage is measured against observed enforcement, never configured intent.
import { setAttr, setData, setHidden } from "../dom.js";
import { compactionPendingLabel, positiveCompactionTokens } from "./compactionPresentation.js";
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
  const observation = agent.compaction;
  const active = observation?.active;
  const threshold = observation?.enforcement === "native" ? active?.translated_trigger
    : observation?.enforcement === "between_turn" ? active?.tokens : null;
  if (positiveCompactionTokens(threshold)) {
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
    title: [`${text} of ${measure.words}, ${percent}% used, as of its last turn`, compactionPendingLabel(agent.compaction)].filter(Boolean).join(". "),
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
