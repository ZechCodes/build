// The app's own frame, written with the SPA's markup and class names so its
// stylesheets lay it out: the inbox rail, the view column with its toolbar,
// directory rail and console bar, and the agent rail with its bubble strip.
// Scenes (workspace-scenes.js, project-scenes.js) only fill the regions;
// nothing here is a marketing layout. The structure follows the live app's
// DOM at each device's width.
import { inboxRowHtml, workspaceEntries } from "../../spa/src/core/inbox.js";
import { toolbarHtml } from "../../spa/src/core/toolbarRender.js";
import { DIRECTORY_TABS, directoryRailHtml } from "../../spa/src/core/directoryRail.js";
import { harnessIconHtml } from "../../spa/src/core/harnessIcon.js";
import { createPatternRenderer } from "../../spa/src/core/agentCanvas.js";
import {
  ICON_ARROW_RIGHT, ICON_CHAT_OVERVIEW, ICON_CHEVRON_DOWN, ICON_EYE, ICON_FOLDERS, ICON_INBOX,
  ICON_PAPERCLIP, ICON_PIN, ICON_PLUS, ICON_SETTINGS,
} from "../../spa/src/core/icons.js";
import { CONVERSATIONS, DEVICE, DIRECTORY, PROJECT, WORKSPACES } from "./story.js";

const PROVIDERS = { claude: "claude_adk", codex: "codex_app_server" };
export const harnessMark = (harness) => harnessIconHtml(PROVIDERS[harness] || harness);

// ---- inbox rail ------------------------------------------------------------

/** Inbox rows for the named workspaces, in the order given, drawn by the
 *  SPA's own row renderer. */
export function inboxRows(names, active) {
  const entries = workspaceEntries(names.map((name) => WORKSPACES[name]), [PROJECT], names.map((name) => CONVERSATIONS[name]));
  const byName = new Map(entries.map((entry) => [entry.name, entry]));
  const activeKey = active ? byName.get(WORKSPACES[active].name)?.key : null;
  return names.map((name) => inboxRowHtml(byName.get(WORKSPACES[name].name), { activeKey })).join("");
}

const inboxRail = (rows) => `<aside id="inbox-rail" aria-label="Inbox">
    <div class="inbox-head">
      <div class="inbox-views" id="inbox-views" role="group" aria-label="Rail view">
        <button class="iconbtn inbox-view on" type="button" aria-pressed="true">${ICON_INBOX}</button>
        <button class="iconbtn inbox-view" type="button" aria-pressed="false">${ICON_FOLDERS}</button>
      </div>
      <button class="inbox-new-project" id="inbox-new-project" type="button">${ICON_PLUS}<span>New project</span></button>
      <button class="iconbtn pinbtn" id="inbox-collapse" type="button" aria-pressed="true">${ICON_PIN}</button>
    </div>
    <div id="inbox-list" data-view="inbox">${rows}</div>
    <div class="inbox-foot">
      <button class="inbox-account" id="nav-account" type="button">${ICON_SETTINGS}</button>
      <div id="devpick"><button class="device-picker-toggle" type="button" aria-expanded="false"><span>${DEVICE}</span><span>${ICON_CHEVRON_DOWN}</span></button></div>
    </div>
  </aside>`;

// The floating inbox toggle: shown only where the rail is away (a phone).
const globalControls = () => `<header id="global-controls">
    <button class="iconbtn" id="inbox-open" type="button" aria-expanded="false"><span class="inbox-open-icon">${ICON_INBOX}</span><span class="inbox-open-count"></span></button>
  </header>`;

// ---- the view column's chrome ---------------------------------------------

/** The toolbar standing in a workspace: back to the project, the workspace
 *  switcher, its directory, and the workspace's Tasks. */
export const workspaceToolbar = (name) => toolbarHtml({
  project: PROJECT.name,
  kind: "workspace",
  label: name,
  directories: [{ sourceId: DIRECTORY, label: DIRECTORY, current: true }],
}).replace(/(data-workspace-tasks[^>]*?) hidden>/, "$1>");

/** The toolbar on a project page: its name, then its pages as tabs, then the
 *  page's verbs at the right. */
export function projectToolbar(tabs, current, verbs = "") {
  const projectTabs = tabs.map((label) => ({ id: label.toLowerCase(), label, current: label === current }));
  return toolbarHtml({ project: PROJECT.name, kind: "project", projectTabs })
    .replace('<span class="tb-verb" id="tb-verb"></span>', `<span class="tb-verb" id="tb-verb">${verbs}</span>`);
}

export const PROJECT_VERBS = `<button class="iconbtn" type="button">${ICON_SETTINGS}</button><button class="iconbtn" type="button">${ICON_PLUS}</button>`;

export const directoryRail = (active, extraTabs = []) =>
  `<nav id="dir-rail" aria-label="Directory views" role="tablist">${directoryRailHtml([...DIRECTORY_TABS, ...extraTabs], active)}</nav>`;

const consoleBar = () => `<div id="console-region" data-size="collapsed"><div class="console"><div class="console-head">
    <button type="button" class="console-bar" id="console-toggle" aria-expanded="false"><span class="console-label">Console</span></button>
    <div class="console-tabs scrollstrip"><button type="button" class="iconbtn console-new">+</button></div>
    <div class="console-controls"></div>
  </div></div></div>`;

const connectionStatus = () => `<div id="connection-status"><button type="button" class="connection-status is-connected" data-state="connected">
    <span class="connection-radiate"></span><span class="connection-centre">1</span></button></div>`;

// ---- agent rail --------------------------------------------------------------

const pin = () => `<button type="button" class="iconbtn pinbtn" aria-pressed="true">${ICON_PIN}</button>`;

/** The conversation panel's head: harness, who, Done, watch, pin and the
 *  surface menu's ⋮. */
export const railHead = ({ harness, who, status = "" }) => `<div class="rail-head">
    ${harnessMark(harness)}
    <span class="rail-who" title="${who}">${who}${status ? ` <span class="rail-who-status">${status}</span>` : ""}</span>
    <button type="button" class="btn mini rail-remove">Done</button>
    <button type="button" class="iconbtn rail-watch watching" aria-pressed="true">${ICON_EYE}</button>
    ${pin()}
    <span class="rail-surface-menu"><div class="splitbtn splitbtn-icon"><button type="button" class="iconbtn caret">⋮</button></div></span>
  </div>`;

export const railComposer = ({ model = "Claude Opus 5.5", effort = "medium", placeholder = "Send a message to this agent…", draft = "" } = {}) =>
  `<div class="rail-composer" id="rail-composer"><div class="thread-composer"><div class="composer attachable">
    <textarea id="railinput" placeholder="${placeholder}" style="height: 36px;">${draft}</textarea>
    <div class="composer-bar">
      <div class="composer-choice-controls">
        <div class="composer-model"><div class="splitbtn"><button type="button" class="btn mini caret">${model}</button></div></div>
        <div class="composer-reasoning"><div class="splitbtn"><button type="button" class="btn mini caret">${effort}</button></div></div>
      </div>
      <span class="hint"></span>
      <div class="composer-actions">
        <button type="button" class="composer-attach">${ICON_PAPERCLIP}</button>
        <div class="composer-send-control"><button type="button" class="btn primary composer-send">${ICON_ARROW_RIGHT}</button></div>
      </div>
    </div>
  </div></div></div>`;

/** An open conversation: the panel beside the strip. `thread` is the SPA's
 *  own thread section (threadHtml). */
export const conversationPanel = ({ head, thread, composer = railComposer() }) =>
  `<div class="rail-panel" id="rail-panel">${railHead(head)}
    <div class="rail-body" id="rail-body" style="--rail-composer-clearance: 92px;">${thread}</div>
    ${composer}
  </div>`;

/** The chat overview at the project's scope: every agent of the work,
 *  grouped by workspace. The app offers "All workspaces" only inside one
 *  workspace's overview, so this head has no way out. */
export const overviewPanel = ({ title = "Agents", sections }) => `<section id="rail-overview" class="rail-overview-content" aria-label="Chat overview">
    <div class="rail-head">
      <span class="rail-who" title="Agents">${title}</span>
      ${pin()}
      <span class="rail-surface-menu"></span>
    </div>
    <div class="rail-overview-list">${sections}</div>
  </section>`;

export const overviewSection = (name, rows, { add = true } = {}) => `<section class="rail-overview-section" aria-label="${name}">
    <div class="rail-overview-section-head"><h2>${name}</h2>${add ? '<button type="button" class="iconbtn rail-overview-add">+</button>' : ""}</div>
    ${rows}
  </section>`;

export const overviewRow = ({ name, snippet, state, attrs = "" }) => `<button type="button" class="rail-overview-row"${attrs}>
    <span class="rail-overview-name">${name}</span>
    <span class="rail-overview-snippet">${snippet}</span>
    <span class="rail-overview-state">${state}</span>
  </button>`;

/** The strip: the project's agent, then this work's agents as pattern
 *  bubbles, the `+`, and the overview control. `agents` are pattern indexes. */
function railStrip({ agents = [], active = null, overview = false } = {}) {
  const bubbles = agents.map((pattern, index) =>
    `<button type="button" class="rail-bubble rail-bubble-agent${index === active ? " active" : ""}" data-bubble="agent" data-pattern="${pattern}" data-seed="agent-${index}"><canvas class="rail-glyph"></canvas></button>`).join("");
  const separator = agents.length ? '<div class="rail-sep" role="separator"></div>' : "";
  const add = agents.length ? '<button type="button" class="rail-bubble rail-bubble-add" data-bubble="add"><span class="rail-bubble-label">+</span></button>' : "";
  return `<div class="rail-strip">
    <button type="button" class="rail-bubble rail-bubble-project${agents.length ? "" : " rail-bubble-ghost active"}" data-bubble="project"><span class="rail-bubble-label">${PROJECT.name[0].toUpperCase()}</span></button>
    ${separator}${bubbles}${add}
    <button type="button" class="rail-overview-toggle" data-bubble="overview" aria-expanded="${overview}">${ICON_CHAT_OVERVIEW}</button>
  </div>`;
}

/** The agent rail in one of its three states: a conversation open beside the
 *  strip, the chat overview open, or collapsed to the strip alone. */
export function agentRail({ panel = null, overview = null, strip = {} }) {
  if (overview) return `<aside id="agent-rail" aria-label="Agents" class="rail-overview">${overview}${railStrip({ ...strip, overview: true })}</aside>`;
  if (panel) return `<aside id="agent-rail" aria-label="Agents">${panel}${railStrip(strip)}</aside>`;
  return `<aside id="agent-rail" aria-label="Agents" class="rail-collapsed">${railStrip(strip)}</aside>`;
}

// ---- the shell ----------------------------------------------------------------

/** The whole frame. `inbox` is a list of workspace names; `dirRail` and
 *  `console` belong to a workspace page, not to a project page. */
export function appShell({ inbox, active, toolbar, dirRail = "", root, rail, console: hasConsole = Boolean(dirRail) }) {
  return `<div class="app">
    <div id="shell">
      ${globalControls()}
      ${inboxRail(inboxRows(inbox, active))}
      <div id="view">
        <div id="toolbar">${toolbar}</div>
        <div id="view-body">
          ${dirRail}
          <main id="root" class="surface"><div id="tabbody" class="flush">${root}</div></main>
          ${rail}
        </div>
        ${hasConsole ? consoleBar() : ""}
        ${connectionStatus()}
      </div>
    </div>
  </div>`;
}

/** The strip's bubbles are canvases the SPA paints; paint them the same way,
 *  once, at rest. */
export function paintBubbles(root) {
  for (const button of root.querySelectorAll(".rail-bubble[data-pattern]")) {
    const renderer = createPatternRenderer({
      canvas: button.querySelector("canvas"),
      patternIndex: Number(button.dataset.pattern),
      seed: button.dataset.seed,
    });
    renderer.setInk(getComputedStyle(button).color);
  }
}
