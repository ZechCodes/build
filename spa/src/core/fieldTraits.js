/**
 * What each text field is, told to the keyboard and to autofill (#139).
 *
 * iOS Safari guesses a field's purpose from its naming when the field does not
 * say, and on a message box it offered "AutoFill Contact" and addresses. Every
 * input and textarea the SPA draws names one of these kinds, so each carries
 * the same explicit set and none is left for the browser to guess. There is no
 * standard autocomplete token for "free text", so every kind is `off`; the rest
 * of the set is what the field is for. `test/fieldTraits.test.js` walks src/ and
 * fails on a field without them.
 */

const KINDS = {
  /** Several lines of writing: messages, comments, notes. Enter adds a line. */
  prose: { autocorrect: "on", autocapitalize: "sentences", spellcheck: "true", inputmode: "text", enterkeyhint: "enter" },
  /** One line of writing a person reads: a task title, an agent's name. */
  line: { autocorrect: "on", autocapitalize: "sentences", spellcheck: "true", inputmode: "text", enterkeyhint: "done" },
  /** Typed exactly as the machine needs it: branches, paths, URLs, model ids. */
  identifier: { autocorrect: "off", autocapitalize: "off", spellcheck: "false", inputmode: "text", enterkeyhint: "done" },
  /** A filter over a list. */
  search: { autocorrect: "off", autocapitalize: "off", spellcheck: "false", inputmode: "search", enterkeyhint: "search" },
  /** A pairing code, read off another screen in capitals. */
  code: { autocorrect: "off", autocapitalize: "characters", spellcheck: "false", inputmode: "text", enterkeyhint: "go" },
};

function traitsOf(kind, enterKey) {
  const traits = KINDS[kind];
  if (!traits) throw new Error(`No field kind "${kind}"`);
  return { autocomplete: "off", ...traits, ...(enterKey ? { enterkeyhint: enterKey } : {}) };
}

/** The attributes for a field drawn in a template. `enterKey` relabels the
 *  Enter key when Enter does something other than the kind's default. */
export function fieldTraits(kind, enterKey) {
  return Object.entries(traitsOf(kind, enterKey))
    .map(([name, value]) => `${name}="${value}"`)
    .join(" ");
}

/** The same attributes on a field built with createElement. */
export function applyFieldTraits(field, kind, enterKey) {
  for (const [name, value] of Object.entries(traitsOf(kind, enterKey))) field.setAttribute(name, value);
}
