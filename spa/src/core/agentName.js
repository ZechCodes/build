// What to call an agent.
//
// Every agent used to be an ordinal — "Agent 1", "Agent 2" — which says where
// it sits in a rail and nothing about what it is. A named agent carries a
// `name` (bridge 1.7.0): one or two meaningful words, set when it was made or
// chosen by the agent itself. Everywhere the client used to print the ordinal
// it prints the name, and falls back to the ordinal for an agent nobody has
// named and for an older bridge that carries no such field.
//
// One module, because a name that reads differently in the rail, in a message
// and on the board is three agents to the reader.

/** The name an agent carries, trimmed, or "" for one that carries none. */
export function agentName(agent) {
  return String(agent?.name || "").trim();
}

/**
 * What to call this agent on its own: its name, or "Agent {ordinal}".
 *
 * `fallbackOrdinal` is for a caller holding a place in a list rather than a
 * digest — the strip's index — and is used only when the agent itself names
 * neither.
 */
export function agentDisplayName(agent, fallbackOrdinal = null) {
  const name = agentName(agent);
  if (name) return name;
  const ordinal = agent?.ordinal || fallbackOrdinal;
  return ordinal ? `Agent ${ordinal}` : "Agent";
}

/**
 * The one or two letters a bubble wears for an agent that has a name.
 *
 * Two words give two initials ("Rail scroll" → "RS"), one word gives one
 * letter. `""` for an unnamed agent, whose bubble keeps the painted pattern it
 * has always worn — a letter cut from an ordinal would say nothing the pattern
 * does not already say better.
 */
export function agentInitials(agent) {
  const words = agentName(agent).split(" ").filter(Boolean);
  if (!words.length) return "";
  return words
    .slice(0, 2)
    .map((word) => [...word][0].toUpperCase())
    .join("");
}
