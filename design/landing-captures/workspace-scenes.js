// The scenes that stand inside the archive-search workspace: the Files view
// with the test open (acts 1 and 2), the Changes view mid-commit (act 6) and
// after the merge (acts 7 and 8), the review triage the tablet shows (act 7,
// a feature the app does not have yet, drawn in its frame), and the phone's
// conversation (act 4).
import { threadHtml } from "../../spa/src/core/thread.js";
import { diffRowsHtml, fileHeadHtml } from "../../spa/src/core/diffRender.js";
import { commitRowHtml } from "../../spa/src/core/changesRender.js";
import { highlightCode, langForPath } from "../../spa/src/core/highlight.js";
import { ICON_CHECK } from "../../spa/src/core/icons.js";
import { agentRail, appShell, conversationPanel, directoryRail, railComposer, workspaceToolbar } from "./app-shell.js";
import { ARCHIVE_DIFF, BRANCH, COMMITS, NOW_SECONDS, TEST_DIFF, TEST_FILE } from "./story.js";

const INBOX_BEFORE = ["search", "terminals", "pairing"];
export const INBOX_TEAM = ["search", "review", "audit", "terminals", "pairing"];
const STRIP_ONE_AGENT = { agents: [1] };

function workspacePage({ inbox = INBOX_TEAM, tab, root, rail = agentRail({ strip: STRIP_ONE_AGENT }), extraTabs = [] }) {
  return appShell({
    inbox,
    active: "search",
    toolbar: workspaceToolbar("archive-search"),
    dirRail: directoryRail(tab, extraTabs),
    root,
    rail,
  });
}

// ---- Files ------------------------------------------------------------------

const treeRow = (name, size, selected = false) =>
  `<div class="frow ffile${selected ? " sel" : ""}"><span class="fk">·</span><span class="fname">${name}</span><span class="fsize mono">${size}</span></div>`;

const sourceRows = (lines, path) => {
  const lang = langForPath(path);
  return lines.map((line, index) =>
    `<tr><td class="fsrc-ln">${index + 1}</td><td class="fsrc-code"><code>${highlightCode(line, lang)}</code></td></tr>`).join("");
};

function filesView() {
  const path = "src/search/archive.test.ts";
  return `<div class="files pane-split">
    <div class="ftree pane-list" id="ftree"><div class="ftree-list">
      <div class="fcrumb mono">src/search</div>
      <div class="frow fup"><span class="fk">↰</span>..</div>
      ${treeRow("archive.test.ts", 122, true)}${treeRow("archive.ts", 214)}${treeRow("index.ts", 96)}${treeRow("search.ts", 1408)}
    </div></div>
    <div class="fpreview" id="fpreview">
      <div class="fphead"><span class="fppath mono">${path}</span><span class="fpsize mono">122 bytes</span>
        <div class="file-mode-tray" role="tablist"><button type="button" class="file-mode active">Source</button><button type="button" class="file-mode">Edit</button></div></div>
      <div class="fpbody file-reading-layer"><div class="fsrc"><table><tbody>${sourceRows(TEST_FILE, path)}</tbody></table></div></div>
    </div>
  </div>`;
}

export const editorScene = () => workspacePage({ inbox: INBOX_BEFORE, tab: "files", root: filesView() });

// ---- Changes ----------------------------------------------------------------

const refRow = (title, sub, selected) =>
  `<div class="rrow${selected ? " sel" : ""}"><span class="rtitle">${title}</span><span class="rsub mono">${sub}</span></div>`;

function changesRail({ selected, pushed, uncommitted, commits }) {
  return `<aside class="crail-host pane-list">
    <div class="workspace-refbar"><div class="workspace-refpicker"><button type="button" class="workspace-reftrigger">
      <span class="workspace-reftrigger-kind">Branch</span><span class="workspace-reftrigger-name">${BRANCH}</span><span>⌄</span></button></div></div>
    <div class="crail">
      ${refRow("All changes", pushed, selected === "all")}
      ${refRow("Uncommitted", uncommitted, selected === "uncommitted")}
      <div class="rhead">Commits</div>
      ${commits.map((commit) => commitRowHtml(commit, { nowSeconds: NOW_SECONDS })).join("")}
    </div>
  </aside>`;
}

const diffFile = (file, rows) => `<div class="file">
    ${fileHeadHtml(file, { selectable: true, openable: true, commentable: true })}
    <div class="dscroll"><table><tbody>${diffRowsHtml(rows, langForPath(file.path))}</tbody></table></div>
  </div>`;

const toolbarVerbs = (stat) => `<div class="gittoolbar"><div class="gtstatus"><span class="gtstat mono"><span style="color:var(--green)">+${stat.add}</span> <span style="color:var(--red)">−${stat.del}</span></span></div>
    <div class="gtrest"><div class="gtsync">
      <button class="btn mini gtfetch">Fetch</button>
      <div class="gtpull"><div class="splitbtn"><button class="btn mini">Pull</button><button class="btn mini caret"><span class="disclosure-caret">▾</span></button></div></div>
      <div class="gtpush"><div class="splitbtn"><button class="btn mini">Push</button><button class="btn mini caret"><span class="disclosure-caret">▾</span></button></div></div>
    </div>
    <div class="gtstash"><div class="splitbtn"><button class="btn mini">Stash</button><button class="btn mini caret"><span class="disclosure-caret">▾</span></button></div></div></div>
  </div>`;

const commitBox = (placeholder) => `<div class="gp-commit"><div class="csbox"><div class="csbox-row">
    <textarea class="csinput" placeholder="${placeholder}" style="height: 37px;"></textarea>
    <div class="csbox-actions"><div class="splitbtn"><button class="btn primary">Comment</button><button class="btn primary caret"><span class="disclosure-caret">▾</span></button></div></div>
  </div></div></div>`;

const ARCHIVE_FILE = { path: "src/search/archive.ts", status: "EDIT", add: 3, del: 1, editedAt: Date.now() - 60000 };
const TEST_FILE_CHANGE = { path: "src/search/archive.test.ts", status: "EDIT", add: 3, del: 0, editedAt: Date.now() - 30000 };

// On a phone the list folds into the handle, which names what is selected.
function changesView({ selected, pushed = "Not pushed yet", uncommitted, commits, detail }) {
  const summary = selected === "all" ? `All changes · ${pushed}` : `Uncommitted · ${uncommitted}`;
  return `<div class="workspace-gitpane"><div class="gitpane"><div class="changes2 pane-split">
    ${changesRail({ selected, pushed, uncommitted, commits })}
    <section class="cdetail">${detail}</section>
    <button class="pane-handle" type="button" aria-expanded="false"><span class="pane-handle-what">${summary}</span><span class="pane-handle-caret disclosure-caret"></span></button>
  </div></div></div>`;
}

const uncommittedDetail = () => `<div class="gp-toolbar">${toolbarVerbs({ add: 6, del: 1 })}</div>
    <div class="cdetail-host">
      <div class="csbar"><div class="diffbar"><span>2 files <span style="color:var(--green)">+6</span> <span style="color:var(--red)">−1</span></span>
        <label class="diffsort"><span>Sort</span><select class="diffsort-select"><option>Latest changes</option></select></label></div></div>
      <div class="dstack">${diffFile(ARCHIVE_FILE, ARCHIVE_DIFF)}${diffFile(TEST_FILE_CHANGE, TEST_DIFF)}</div>
    </div>
    ${commitBox("Comment on these changes, or write a commit message…")}`;

export const gitScene = () => workspacePage({
  tab: "changes",
  root: changesView({ selected: "uncommitted", uncommitted: "+6 −1", commits: COMMITS, detail: uncommittedDetail() }),
});

const MERGED_COMMITS = [
  { hash: "e7a31c0", short: "e7a31c0", subject: "Merge #78: make archived items searchable", author: "Build", time: NOW_SECONDS - 20 },
  { hash: "b2f94d1", short: "b2f94d1", subject: "archive: keep archived results searchable", author: "you", time: NOW_SECONDS - 140 },
  ...COMMITS,
];

// The agent's hunk with the line the visitor typed in act 6.
const MERGED_ARCHIVE_DIFF = [
  ...ARCHIVE_DIFF.slice(0, 5),
  { t: "add", n: 14, text: '    label: "Archived",' },
  { t: "add", n: 15, text: "  })" },
];

const mergedDetail = () => `<div class="gp-toolbar"><div class="gittoolbar"><div class="gtstatus"></div>
      <div class="gtrest"><div class="gitstate gitstate-merged"><span class="gitstate-msg">${ICON_CHECK} Merged into main · archive behavior updated</span></div></div></div></div>
    <div class="cdetail-host">
      <div class="csbar"><div class="diffbar"><span>2 files <span style="color:var(--green)">+7</span> <span style="color:var(--red)">−1</span></span>
        <label class="diffsort"><span>Sort</span><select class="diffsort-select"><option>Latest changes</option></select></label></div></div>
      <div class="dstack">${diffFile({ ...ARCHIVE_FILE, add: 4 }, MERGED_ARCHIVE_DIFF)}${diffFile(TEST_FILE_CHANGE, TEST_DIFF)}</div>
    </div>`;

export const mergedScene = () => workspacePage({
  tab: "changes",
  root: changesView({ selected: "all", pushed: "Merged", uncommitted: "clean", commits: MERGED_COMMITS, detail: mergedDetail() }),
});

// ---- Review triage (not in the app yet) ---------------------------------------

const REVIEW_TAB = { id: "review", label: "Review", icon: ICON_CHECK };

const triageRow = ({ key, count, title, detail = "", open = false, body = "", tail = '<span class="triage-dot"></span>' }) => `<li class="triage-row${open ? " open" : ""}" data-triage-row="${key}">
    <div class="triage-row-head"><span class="triage-chevron">${open ? "⌄" : "›"}</span>
      <span class="triage-row-text"><span class="triage-kind">${count}</span><span class="triage-title">${title}</span>${detail ? `<span class="triage-detail">${detail}</span>` : ""}</span>
      ${tail}</div>
    ${body}
  </li>`;

const FINDING_DIFF = [ARCHIVE_DIFF[2], ARCHIVE_DIFF[3], ARCHIVE_DIFF[4], { t: "add", n: 14, text: '    label: "Archived",' }, { t: "add", n: 15, text: "  })" }];

const approvalHtml = (state) => (state === "merged"
  ? `<span class="triage-approved">${ICON_CHECK} Approved for merge · merged into main</span>`
  : '<button class="btn primary">Approve for merge</button>');

// The finding is open from the start; once a person is reading it, it shows
// its diff and the approval, and the passing rows fold into one.
function findingRow(state) {
  const summary = `<p class="triage-summary">Archived results remain visible and now carry a clear label.</p>
    <p class="triage-evidence mono">archive.test.ts passed · keeps archived items in search ✓</p>`;
  const decision = state === "triage" ? "" : `<div class="triage-finding">
      <div class="file"><div class="dscroll"><table><tbody>${diffRowsHtml(FINDING_DIFF, "typescript")}</tbody></table></div></div>
      <div class="triage-approval">${approvalHtml(state)}</div>
    </div>`;
  return triageRow({ key: "finding", count: "Needs you · 1", title: "Search index behaviour changed", open: true, body: summary + decision });
}

const passingRows = (state) => (state === "triage"
  ? triageRow({ key: "verified", count: "Verified · 3", title: "Checks passed", detail: "archive.test.ts · search.test.ts · lint", tail: "" })
    + triageRow({ key: "failed", count: "Failed · 0", title: "No failing checks", tail: "" })
  : triageRow({ key: "rest", count: "Verified · 3 &nbsp;·&nbsp; Failed · 0", title: "Everything else passed", tail: '<span class="triage-count">3</span>' }));

function triageView(state) {
  const approved = state === "merged";
  return `<div class="review-triage">
    <div class="triage-head"><div><p class="triage-kicker">Ready for review</p><h1>Review triage</h1></div>
      <span class="triage-pill${approved ? " done" : ""}">${approved ? "Approved" : "1 needs you"}</span></div>
    <ul class="triage-list">${findingRow(state)}${passingRows(state)}</ul>
  </div>`;
}

export const triageScene = (state = "triage") => workspacePage({
  tab: "review",
  extraTabs: [REVIEW_TAB],
  root: triageView(state),
});

// ---- The conversation, on a phone ---------------------------------------------

function conversationItems(state) {
  const items = [
    { type: "message", data: { role: "user", body: "Make archived items searchable, #78.", delivery_status: "seen" } },
    { type: "event", data: { event: "run_started", summary: "Inspected archive search behavior" } },
    { type: "message", data: { role: "agent", body: "I found the archive action removing the item from search. Keep archived items in search?" } },
  ];
  if (state !== "question") items.push({ type: "message", data: { role: "user", body: "Yes. Label them clearly.", delivery_status: "seen" } });
  if (state === "resumed") {
    items.push(
      { type: "event", data: { event: "stage_started", summary: "Editing archive behavior and adding coverage" } },
      { type: "message", data: { role: "agent", body: "I’ll keep the search entry with an archived flag, show an Archived label in results, and run the focused checks." } },
    );
  }
  return items;
}

export function conversationScene(state) {
  const panel = conversationPanel({
    head: { harness: "claude", who: "Implement" },
    thread: threadHtml({ items: conversationItems(state) }, { agentLabel: "Claude Code" }),
    composer: railComposer({ placeholder: state === "question" ? "Answer Implement…" : "Send a message to this agent…" }),
  });
  return workspacePage({
    tab: "changes",
    root: changesView({ selected: "uncommitted", uncommitted: "+6 −1", commits: COMMITS, detail: uncommittedDetail() }),
    rail: agentRail({ panel, strip: { agents: [1], active: 0 } }),
  });
}
