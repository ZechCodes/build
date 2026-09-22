import { inboxRowHtml, workspaceEntries } from "../../spa/src/core/inbox.js";
import { agentRowHtml, checklistItemHtml, shellRowHtml } from "../../spa/src/core/agentSurfacesRender.js";
import { gitToolbarHtml } from "../../spa/src/core/gitRender.js";
import { diffRowsHtml } from "../../spa/src/core/diffRender.js";
import { directoryRailHtml, DIRECTORY_TABS } from "../../spa/src/core/directoryRail.js";
import { threadHtml } from "../../spa/src/core/thread.js";
import { ICON_FOLDERS, ICON_INBOX } from "../../spa/src/core/icons.js";
import { renderSystemShell } from "./system-ui.js";

const query = new URLSearchParams(location.search);
const scene = query.get("scene") || "ui01";
const state = query.get("state") || "default";
const profile = query.get("profile") || "app";

const icons = {
  claude: "../../spa/src/assets/harnesses/claude.svg",
  codex: "../../spa/src/assets/harnesses/codex.svg",
  pi: "../../spa/src/assets/harnesses/pi.svg",
};

const mark = (kind = "running") => `<span class="state-mark ${kind}" aria-hidden="true"></span>`;
const harness = (name, kind = "claude") => `<span class="harness-icon"><img src="${icons[kind]}" alt=""></span>${name}`;

const launchWorkspace = {
  id: "workspace-launch",
  workspaceKey: "workshop/workspace-launch",
  project_id: "project-build",
  projectKey: "workshop/project-build",
  deviceId: "workshop",
  entity_id: "run-archive",
  name: "Launch",
  root: "/workspaces/Launch",
  status: "active",
  work_summary: { pushes: 2, additions: 128, deletions: 34, clean: false },
  directories: [
    { source_id: "build-app", name: "build-app", is_git: true },
    { source_id: "build-bridge", name: "build-bridge", is_git: true },
    { source_id: "notes", name: "notes", is_git: false },
  ],
  created_at: "2026-09-17T20:00:00Z",
  updated_at: "2026-09-17T20:03:43Z",
};
const launchProject = { id: "project-build", project_id: "project-build", projectKey: "workshop/project-build", name: "Build" };
const launchConversation = {
  kind: "branch",
  entity_id: "run-archive",
  project_id: "project-build",
  projectKey: "workshop/project-build",
  branch: "main",
  title: "Fixing archive button",
  working: true,
  unread: true,
  unread_count: 1,
};
const launchEntry = workspaceEntries([launchWorkspace], [launchProject], [launchConversation])[0];
const launchRow = inboxRowHtml(launchEntry, { activeKey: launchEntry.key })
  .replace(/<span class="inbox-tag">[\s\S]*?<\/span>/, "")
  .replace(/<span class="badge inbox-unread">[\s\S]*?<\/span>/, "");

function sidebar(active = "Launch", timer = "03:43") {
  return `<aside class="sidebar">
    <div class="view-switch" aria-label="Inbox mode"><span class="active" title="Inbox">${ICON_INBOX}</span><span title="Projects">${ICON_FOLDERS}</span></div>
    <div class="side-label">Inbox</div>
    <div class="actual-inbox ${active === "Launch" ? "selected" : ""}">${launchRow}<span class="capture-row-time">${timer}</span></div>
    <div class="side-bottom"><div class="side-row"><span class="side-icon">◎</span><div class="side-title">Settings</div></div></div>
  </aside>`;
}

function repoTabs(active = "build-app") {
  const tabs = [
    ["build-app", "Git · +128 −34"],
    ["build-bridge", "Git · clean"],
    ["notes", "Non-Git directory"],
  ];
  return `<div class="repo-tabs">${tabs.map(([name, status]) => `<div class="repo-tab ${name === active ? "active" : ""}"><strong>${name}</strong><span>${status}</span></div>`).join("")}</div>`;
}

function head(title, subtitle, time = "03:43", provider = null) {
  return `<div class="surface-head">
    <div class="titleblock">
      <div class="surface-title">${provider ? harness(title, provider) : title}</div>
      <div class="surface-sub">${subtitle}</div>
    </div>
    <div class="head-actions"><span class="pill running">Running</span><span class="timer">${time}</span></div>
  </div>`;
}

function shell(content, { active = "Launch", crumb = "Launch", timer = "03:43" } = {}) {
  const machineLabel = profile === "macbook"
    ? '<span class="machine-label">zech-mbp · ~/code/build · main</span>'
    : "";
  return `<div class="app">
    <header class="global-head"><div class="brand"><img src="./build-mark.svg" alt=""><span>build</span></div><div class="crumbs"><span>Build</span><span class="crumb-sep">/</span><strong>${crumb}</strong><div class="head-status"><span class="presence"></span><span>workshop online</span><span class="avatar">Z</span></div></div></header>
    ${sidebar(active, timer)}
    <main class="main">${content}</main>
    <footer class="statusbar"><span><span class="presence"></span>&nbsp; Connected to workshop</span>${machineLabel}<span>2 sub-agents</span><span>2 shells</span><span>${timer} · <span class="git-add">+128</span> <span class="git-del">−34</span></span></footer>
  </div>`;
}

function ui01() {
  const surfaceAgents = [
    ["Fixing archive button", "Claude Code", "claude"],
    ["Keyboard review", "Codex", "codex"],
    ["Planning next steps", "Codex", "codex"],
  ];
  const body = `${head("Launch", "Build · workspace", "03:42")}${repoTabs()}
    <div class="workspace">
      <nav class="dir-rail actual-dir-rail">${directoryRailHtml(DIRECTORY_TABS, "changes")}</nav>
      <section class="tab-body"><div class="delegation">
        <div class="kicker">Delegate work</div>
        <h1 class="request">Fix the archive button and keep archived results searchable.</h1>
        <p class="request-detail">Inspect the current behavior, patch the archive flow, add a clear archived label, and verify the result.</p>
        <div class="choice-grid">
          <div class="choice selected"><div class="choice-head">${harness("Claude Code", "claude")}<span class="pill done" style="margin-left:auto">Selected</span></div><p>Implementation and repository work</p></div>
          <div class="choice"><div class="choice-head">${harness("Codex", "codex")}</div><p>Review and focused changes</p></div>
          <div class="choice"><div class="choice-head">${harness("Pi", "pi")}</div><p>Lightweight command-line work</p></div>
        </div>
        <div class="started">
          <div class="started-row">${mark()}<div><strong>Fixing archive button</strong> <small>Claude Code</small></div><span class="agent-state running">Running</span></div>
          <div class="started-row">${mark()}<div><strong>Keyboard review</strong> <small>Codex</small></div><span class="agent-state running">Running</span></div>
          <div class="started-row">${mark()}<div><strong>Planning next steps</strong> <small>Codex</small></div><span class="agent-state running">Running</span></div>
        </div>
      </div></section>
      <aside class="agent-rail"><div class="rail-section-title">Agents <span>3</span></div>
        ${surfaceAgents.map(([topic, provider, icon]) => `<div class="agent-card"><div class="agent-head">${mark()}<span class="agent-name">${topic}</span><span class="agent-state running">Working</span></div><div class="agent-role">${harness(provider, icon)}</div></div>`).join("")}
      </aside>
    </div>`;
  return shell(body, { timer: "03:42" });
}

function ui02() {
  const body = `${head("Inbox", "Workspace activity across your devices", "03:43")}
    <section class="inbox-content">
      <div class="kicker">In progress</div>
      <h1 class="request" style="font-size:26px">Pick up where you left off.</h1>
      <p class="inbox-intro">The same Launch workspace is running on your personal host.</p>
      <div class="inbox-card actual-inbox main-inbox">${launchRow}<span class="capture-row-time">03:43</span></div>
    </section>`;
  return shell(body, { timer: "03:43" });
}

function ui03() {
  const items = [
    { type: "event", data: { event: "run_started", summary: "Inspected archive search behavior" } },
    { type: "message", data: { role: "agent", body: "I found the archive action removing the item from search. Keep archived items in search?" } },
  ];
  if (state === "answer" || state === "resumed" || state === "default") {
    items.push({ type: "message", data: { role: "user", body: "Yes. Label them clearly.", delivery_status: "seen" } });
  }
  if (state === "resumed" || state === "default") {
    items.push(
      { type: "event", data: { event: "stage_started", summary: "Editing archive behavior and adding coverage" } },
      { type: "message", data: { role: "agent", body: "I’ll preserve the search entry with an archived flag, show a visible Archived label in results, and run the focused checks." } },
    );
  }
  const conversation = threadHtml({ items });
  const body = `${head("Fixing archive button", "Launch · build-app", "03:44", "claude")}
    <section class="chat">
      <div class="messages actual-thread">${conversation}</div>
      <div class="composer"><div class="composer-row"><span class="composer-placeholder">Message the agent…</span><button class="send">↑</button></div><div class="composer-tools"><span>＋ Attach</span><span>${harness("Claude Code", "claude")}</span></div></div>
    </section>`;
  return shell(body, { active: "Fixing archive button", crumb: "Fixing archive button", timer: "03:44" });
}

const rowState = (kind, label) => ({ mark: kind === "done" ? "ok" : "running", label });
const renderedAgent = (key, subject, kind, label) => agentRowHtml({ key, subject, stateMark: rowState(kind, label), clock: label, tail: [] });
const renderedStep = (key, subject, kind, label) => checklistItemHtml({ key, subject, stateMark: rowState(kind, label), clock: label, description: subject });
const renderedShell = (key, subject, label) => shellRowHtml({ key, subject, stateMark: rowState("running", label), clock: label, tail: [] });

function ui04() {
  const body = `${head("Launch", "Activity overview · Build", "03:44")}
    <section class="activity">
      <div class="activity-canvas">
        <div class="kicker">Workspace overview</div><h1 class="request" style="font-size:28px">Everything moving in one place.</h1>
        <div class="activity-summary"><div class="stat-card"><b>2</b><span>Sub-agents</span></div><div class="stat-card"><b>1</b><span>Workflow</span></div><div class="stat-card"><b>2</b><span>Shells</span></div></div>
        <div class="latest"><h3>Latest action</h3><div class="latest-line">${mark("running")} Verify archive result stays visible and labeled <span class="latest-time">now</span></div><div class="latest-line">${mark("done")} Patch archive update behavior <span class="latest-time">1m</span></div><div class="latest-line">${mark("done")} Inspect search index flow <span class="latest-time">2m</span></div></div>
      </div>
      <aside class="activity-panel">
        <div class="panel-title">Activity <span>03:44</span></div>
        <div class="group"><div class="group-head"><span>Sub-agents</span><span>2</span></div>${renderedAgent("keyboard-review", "Keyboard review", "running", "Running")}${renderedAgent("edge-case-audit", "Edge-case audit", "done", "Done")}</div>
        <div class="group"><div class="group-head"><span>Workflow</span><span>2 / 3</span></div>${renderedStep("inspect", "Inspect", "done", "Done")}${renderedStep("patch", "Patch", "done", "Done")}${renderedStep("verify", "Verify", "running", "Running")}</div>
        <div class="group"><div class="group-head"><span>Shells</span><span>2</span></div>${renderedShell("tests", "tests", "Running")}${renderedShell("dev-server", "dev server", "Listening")}</div>
      </aside>
    </section>`;
  return shell(body, { timer: "03:44" });
}

function ui05() {
  const merged = state === "merged";
  const renderedDiff = diffRowsHtml([
    { t: "hunk", text: "@@ archiveItem(id: string) @@" },
    { t: "ctx", o: 12, n: 12, text: "const item = search.get(id)" },
    { t: "del", o: 13, text: "search.remove(id)" },
    { t: "add", n: 13, text: "search.update(id, {" },
    { t: "add", n: 14, text: "  archived: true," },
    { t: "add", n: 15, text: "  label: \"Archived\"," },
    { t: "ctx", o: 14, n: 16, text: "})" },
    { t: "hunk", text: "@@ search results @@" },
    { t: "add", n: 32, text: "result.badge = item.archived ? \"Archived\" : null" },
  ], "javascript");
  const toolbar = gitToolbarHtml({ chips: { ahead: 2, behind: 0 }, stat: { insertions: 23, deletions: 1 }, repo: false });
  const body = `${head("Changes", "Launch · build-app · review", "03:44")}${repoTabs()}
    <section class="review">
      <aside class="files"><div class="file-head">3 files changed</div><div class="file-row active"><span>◇</span>archive.ts <span class="git-add">+3 −1</span></div><div class="file-row"><span>◇</span>search.ts <span class="git-add">+6</span></div><div class="file-row"><span>◇</span>archive.test.ts <span class="git-add">+14</span></div></aside>
      <div class="diff">${toolbar}<div class="diff-head"><span>archive.ts</span><span class="counts"><span class="git-add">+3</span><span class="git-del">−1</span></span></div><div class="code actual-diff"><table><tbody>${renderedDiff}</tbody></table></div></div>
      <aside class="review-side">
        <div class="summary-box"><div class="box-label">Summary</div><p>Archived items remain searchable and receive a visible Archived label.</p></div>
        <div class="evidence-box"><div class="box-label">Evidence</div><p><strong>archive.test.ts passed</strong></p><div class="result-chip">keeps archived items in search ✓</div></div>
        <div class="approval"><div class="approval-note">Yes. Label them clearly.</div>${merged ? '<div class="merged">✓ Merged · Archive behavior updated</div>' : '<button class="merge-button">Approve & merge</button>'}</div>
      </aside>
    </section>`;
  return shell(body, { crumb: "Review changes", timer: "03:44" });
}

function ui10() {
  const body = `${head("archive.ts", "build-app/src/search · editor", "03:45")}
    <section class="editor-layout">
      <aside class="file-tree"><div class="panel-eyebrow">Explorer</div><strong>build-app</strong><div class="tree-path">src / search</div><div class="tree-file active">archive.ts</div><div class="tree-file">archive.test.ts</div><div class="tree-file">search.ts</div></aside>
      <div class="editor-pane"><div class="editor-tabs"><span class="active">archive.ts</span><span>archive.test.ts</span></div><div class="editor-code mono"><div><i>10</i><span>export function archiveItem(id: string) {</span></div><div><i>11</i><span>  const item = search.get(id)</span></div><div class="focus-line"><i>12</i><span>  search.remove(id)</span></div><div><i>13</i><span>}</span></div></div><div class="terminal-collapsed"><span>›_</span> Terminal <b>⌃</b></div></div>
      <aside class="agents-pane"><div class="panel-eyebrow">Agents <span>1</span></div><div class="story-agent"><div>${mark()}<strong>Fixing archive button</strong></div><p>${harness("Claude Code", "claude")}</p><span>Working</span></div></aside>
    </section>`;
  return shell(body, { crumb: "Editor", timer: "03:45" });
}

function ui12() {
  const columns = [
    ["Backlog", "1", '<div class="issue-card muted"><small>#76 · docs</small><strong>Explain local host pairing</strong></div>'],
    ["Ready", "1", '<div class="issue-card featured"><small>#78 <em>search</em> <em>bug</em></small><strong>Make archived items searchable</strong><p>No assignee</p></div>'],
    ["In progress", "1", '<div class="issue-card muted"><small>#74 · shell</small><strong>Persist terminal sessions</strong></div>'],
    ["In review", "1", '<div class="issue-card muted"><small>#73 · git</small><strong>Show branch divergence</strong></div>'],
  ];
  const body = `${head("Issues", "Build · workspace board", "03:46")}<section class="issue-board">${columns.map(([title, count, cards]) => `<div class="issue-column"><header><strong>${title}</strong><span>${count}</span></header>${cards}</div>`).join("")}</section>`;
  return shell(body, { crumb: "Issues", timer: "03:46" });
}

function ui13() {
  const rows = [
    ["Implement", "Claude Code", "claude", "~/code/build/workspaces/archive-search"],
    ["Review", "Codex", "codex", "~/code/build/workspaces/archive-review"],
    ["Audit edge cases", "Codex", "codex", "~/code/build/workspaces/archive-audit"],
  ];
  const body = `${head("Agents", "Archive search · 3 running", "03:47")}<section class="team-layout"><div class="team-main"><div class="team-summary"><div><span class="presence"></span><strong>3 running</strong></div><p>One issue moving across isolated workspaces on this machine.</p></div><div class="team-rows">${rows.map(([task, name, icon, path]) => `<article class="team-row"><div class="team-row-title">${mark()}<strong>${task}</strong><span>Working</span></div><div class="team-meta">${harness(name, icon)}<code>${path}</code></div></article>`).join("")}</div></div><aside class="terminal-drawer"><div class="panel-eyebrow">Terminal · tests</div><code><span>$</span> pnpm test archive.test.ts <b>PASS</b> src/search/archive.test.ts</code></aside></section>`;
  return shell(body, { crumb: "Agents", timer: "03:47" });
}

function ui16() {
  const nodes = [
    ["Implement", "Claude Code", "Done · handoff pending", "done"],
    ["Review", "Codex", "Waiting", ""],
    ["You decide", "Human gate", "Waiting", "gate"],
    ["Merge", "Build", "Waiting", ""],
  ];
  const body = `${head("Fix with tests", "Workflow builder · saved", "03:48")}<section class="builder"><aside class="node-palette"><div class="panel-eyebrow">Node palette</div>${["Agent", "Test", "Review", "Human gate", "Fan out"].map((label) => `<div class="palette-item"><span>＋</span>${label}</div>`).join("")}</aside><div class="workflow-canvas"><div class="canvas-label">Archive search workflow</div><div class="workflow-path">${nodes.map(([title, owner, status, kind], index) => `${index ? '<div class="edge">→</div>' : ""}<article class="workflow-node ${kind}"><small>${index + 1}</small><strong>${title}</strong><span>${owner}</span><em>${status}</em></article>`).join("")}</div></div></section>`;
  return shell(body, { crumb: "Workflow builder", timer: "03:48" });
}

function ui14() {
  const body = `${head("archive.ts", "build/archive-search · ~/code/build/workspaces/archive-search", "03:49")}<section class="git-layout"><aside class="git-panel"><div class="panel-eyebrow">Source control</div><div class="branch-chip">⑂ build/archive-search</div><code class="workspace-path">~/code/build/workspaces/archive-search</code><div class="panel-eyebrow spaced">Graph</div><div class="commit-line active"><span>●</span><strong>Archive search behavior</strong><small>Claude Code</small></div><div class="commit-line"><span>●</span><strong>Review archive coverage</strong><small>Codex</small></div><div class="commit-line"><span>●</span><strong>Audit edge cases</strong><small>Codex</small></div><div class="commit-line main"><span>●</span><strong>main</strong></div><div class="panel-eyebrow spaced">Working tree <span>2</span></div><div class="changed-file active">M archive.ts</div><div class="changed-file">M archive.test.ts</div></aside><div class="git-editor"><div class="editor-tabs"><span class="active">archive.ts</span><span>archive.test.ts</span></div><div class="diff-title"><strong>archive.ts</strong><span class="git-add">+2</span><span class="git-del">−1</span></div><div class="editor-code mono diff-code"><div><i>11</i><span>  const item = search.get(id)</span></div><div class="removed"><i>12</i><span>− search.remove(id)</span></div><div class="added"><i>12</i><span>+ search.update(id, {</span></div><div class="added"><i>13</i><span>+   archived: true</span></div><div class="added"><i>14</i><span>+ })</span></div></div></div></section>`;
  return shell(body, { crumb: "Git", timer: "03:49" });
}

function ui15() {
  const body = `${head("Review", "Archive search · triage", "03:50")}<section class="triage"><div class="triage-heading"><div><div class="kicker">Ready for your call</div><h1>Review triage</h1></div><span class="pill running">1 needs you</span></div><article class="triage-row expanded"><header><span class="chevron">⌄</span><div><small>Needs you · 1</small><strong>Search index behaviour changed</strong></div><span class="attention-dot"></span></header><p>Archived results remain visible and now carry a clear label.</p><code>archive.test.ts passed · keeps archived items in search ✓</code></article><article class="triage-row"><header><span class="chevron">›</span><div><small>Verified · 3</small><strong>Checks passed</strong></div><span class="count-good">3</span></header></article><article class="triage-row"><header><span class="chevron">›</span><div><small>Failed · 0</small><strong>No failing checks</strong></div><span>0</span></header></article></section>`;
  return shell(body, { crumb: "Review triage", timer: "03:50" });
}

const scenes = {
  ui01, ui02, ui03, ui04, ui05,
  "ui10-editor": ui10,
  "ui12-issues": ui12,
  "ui13-team": ui13,
  "ui14-git": ui14,
  "ui15-triage": ui15,
  "ui16-builder": ui16,
};
const appHtml = (scenes[scene] || ui01)();
document.querySelector("#capture").innerHTML = renderSystemShell(profile, appHtml);
