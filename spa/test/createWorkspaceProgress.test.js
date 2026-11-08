// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createWorkspaceProgressModel, openCreateWorkspaceProgress } from "../src/core/createWorkspaceProgress.js";
vi.mock("../src/core/motion.js", () => ({ reveal: async e => { e.hidden = false; }, hide: async e => { e.hidden = true; }, settleHidden: () => {} }));
const flush = async () => { await Promise.resolve(); await Promise.resolve(); };
const operation = (fields = {}) => ({ operationId: "attempt-1", request: { method: "environment.create_project", params: { name: "Release" } }, state: "executing", ...fields });
const workspace = { id: "ws-1", project_id: "p1", name: "Release", directories: [] };
const project = { id: "p1", name: "Payments" };
const completed = () => operation({ state: "succeeded", confirmation: { result: { project, workspace } } });
const failed = () => operation({ state: "failed", effect: "partial", confirmation: { error: { message: "checkout refused", details: { project } } } });
function serviceFor(initial) {
  let row = initial; const listeners = new Set();
  return { readOperations: () => row ? [row] : [], get: vi.fn(async () => row),
    subscribe: listener => { listeners.add(listener); return () => listeners.delete(listener); },
    update(next) { row = next; listeners.forEach(listener => listener()); }, listenerCount: () => listeners.size };
}
let dialogs;
function open(row, extra = {}) {
  const writeOperations = serviceFor(row);
  const dialog = openCreateWorkspaceProgress({ operationId: "attempt-1", deviceId: "laptop", targetSummary: "Laptop · Sources › Payments", writeOperations, autoOpenDelay: 0, ...extra });
  dialogs.push(dialog); return { ...dialog, writeOperations };
}
beforeEach(() => { document.body.innerHTML = ""; dialogs = []; });
afterEach(async () => { await Promise.all(dialogs.map(dialog => dialog.close())); vi.useRealTimers(); });
describe("workspace creation progress", () => {
  it("paints cached Done with the target summary and opens the workspace", async () => {
    const onOpenWorkspace = vi.fn(); open(completed(), { onOpenWorkspace });
    expect(document.querySelector('[role="dialog"]').getAttribute("aria-labelledby")).toBe("create-workspace-progress-title");
    expect(document.body.textContent).toContain("Laptop · Sources › Payments"); expect(document.body.textContent).toContain("Done");
    const button = document.querySelector('[data-progress-action="open"]'); expect(document.activeElement).toBe(button); button.click(); await flush();
    expect(onOpenWorkspace).toHaveBeenCalledWith(expect.objectContaining({ operationId: "attempt-1", workspace, project }));
  });
  it("shows a failed landed project and offers the correct recovery actions", async () => {
    const onDiscardProject = vi.fn(); open(failed(), { onDiscardProject });
    expect(document.body.textContent).toContain("Project Payments was created. Its workspace was not: checkout refused");
    expect(document.querySelector('[data-progress-action="retry"]').textContent).toBe("Try the workspace again");
    expect(document.querySelector('[data-progress-action="keep"]').textContent).toBe("Keep it");
    document.querySelector('[data-progress-action="discard"]').click(); await flush();
    expect(onDiscardProject).toHaveBeenCalledWith(expect.objectContaining({ operationId: "attempt-1", project }));
  });
  it("passes the prior operation and landed project to workspace retry", async () => {
    const onRetryWorkspace = vi.fn(); open(failed(), { onRetryWorkspace });
    document.querySelector('[data-progress-action="retry"]').click(); await flush();
    expect(onRetryWorkspace).toHaveBeenCalledWith(expect.objectContaining({ operationId: "attempt-1", project }));
  });
  it("keeps unknown outcomes Checking without retry or discard", () => {
    open(operation({ state: "outcome_unknown", unresolved: true }));
    expect(document.body.textContent).toContain("Checking…"); expect(document.body.textContent).toContain("Build lost the reply; checking what happened");
    expect(document.querySelector('[data-progress-action="retry"]')).toBeNull(); expect(document.querySelector('[data-progress-action="discard"]')).toBeNull();
  });
  it.each(["unknown", "reconciling"])("paints nested creation state %s as Checking without recovery actions", state => {
    open(operation({ creation: { state }, progress: [{ step: "workspace", state: "done" }] }));
    expect(document.querySelector("h3").textContent).toBe("Checking…");
    expect(document.querySelector('[data-progress-action="open"]')).toBeNull();
    expect(document.querySelector('[data-progress-action="retry"]')).toBeNull();
    expect(document.querySelector('[data-progress-action="discard"]')).toBeNull();
  });
  it("restores an attempt by operation id after reload and responds to cached updates", async () => {
    const held = open(operation({ state: "outcome_unknown" })); await held.ready;
    expect(held.writeOperations.get).toHaveBeenCalledWith("attempt-1"); held.writeOperations.update(completed());
    expect(document.body.textContent).toContain("Done"); expect(document.querySelector('[data-progress-action="open"]')).not.toBeNull();
  });
  it("Close and Escape detach the listener without mutating the operation", async () => {
    const onClose = vi.fn(); const held = open(operation(), { onClose }); expect(held.writeOperations.listenerCount()).toBe(1);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })); await flush();
    expect(held.writeOperations.listenerCount()).toBe(0); expect(onClose).toHaveBeenCalledTimes(1);
    held.writeOperations.update(completed()); expect(document.querySelector('[role="dialog"]')).toBeNull();
  });
  it("failure before any step landed restores the wizard draft", async () => {
    const onBack = vi.fn(); open(operation({ state: "failed", confirmation: { error: "permission refused", effect: "unapplied" } }), { onBack });
    expect(document.body.textContent).toContain("Nothing was created."); document.querySelector('[data-progress-action="back"]').click(); await flush();
    expect(onBack).toHaveBeenCalledWith(expect.objectContaining({ operationId: "attempt-1" }));
  });
  it("uses a single workspace step for an existing project", () => {
    const model = createWorkspaceProgressModel(operation({ request: { method: "workspace.create", params: { project_id: "p1" } } }));
    expect(model.steps.map(step => step.label)).toEqual(["Creating workspace"]); expect(model.status).toBe("Creating…");
  });
  it("paints steps, copies and verbatim failure text safely", () => {
    open(operation({ progress: [{ step: "project", state: "done" }, { step: "workspace", state: "running", detail: "Copying <source>" }] }));
    expect(document.querySelector('[data-progress-step="project"]').dataset.state).toBe("done");
    expect(document.body.textContent).toContain("Copying <source>"); expect(document.querySelector("source")).toBeNull();
  });
  it("does not offer a new attempt while resources need cleanup", () => {
    open(operation({ state: "outcome_unknown", progress: [{ step: "workspace", state: "failed", detail: "Needs cleanup: /work/leftover" }] }));
    expect(document.body.textContent).toContain("Needs cleanup"); expect(document.querySelector('[data-progress-action="retry"]')).toBeNull();
  });
  it("marks a refused fixed-project workspace step failed without inventing a project", () => {
    const model = createWorkspaceProgressModel(operation({ request: { method: "workspace.create", params: { project_id: "p1" } }, state: "failed", effect: "unapplied", confirmation: { error: "checkout refused" } }));
    expect(model.steps).toEqual([expect.objectContaining({ step: "workspace", state: "failed" })]);
    expect(model.project).toBeNull();
  });
  it("retains a discard refusal and its existing recovery actions", async () => {
    open(failed(), { onDiscardProject: vi.fn(async () => { throw new Error("Project now has tasks"); }) });
    document.querySelector('[data-progress-action="discard"]').click(); await flush();
    expect(document.querySelector('[role="alert"]').textContent).toBe("Project now has tasks");
    expect(document.querySelector('[data-progress-action="retry"]').disabled).toBe(false);
  });
  it("does not steal outside focus when a cached update finishes provisioning", async () => {
    vi.useFakeTimers(); const onOpenWorkspace = vi.fn();
    const held = open(operation(), { onOpenWorkspace, autoOpenDelay: 900 }); await held.ready;
    const elsewhere = document.createElement("button"); document.body.append(elsewhere); elsewhere.focus();
    held.writeOperations.update(completed()); await vi.advanceTimersByTimeAsync(900);
    expect(document.activeElement).toBe(elsewhere); expect(onOpenWorkspace).not.toHaveBeenCalled();
  });
  it("keeps a failed workspace in an acceptance payload nonterminal", () => {
    const model = createWorkspaceProgressModel(operation({ state: "accepted", ack: { result: { workspace: { workspace_id: "w1", project_id: "p1", status: "failed" } } } }));
    expect(model.phase).toBe("creating");
    expect(model.actions).toEqual([]);
  });
  it("preserves the bare workspace.create wire fields for Open workspace", async () => {
    const result = { workspace_id: "wire-ws", project_id: "p1", name: "Wire", directories: [] };
    const onOpenWorkspace = vi.fn();
    open(operation({ state: "succeeded", request: { method: "workspace.create", params: {} }, confirmation: { result } }), { onOpenWorkspace });
    document.querySelector('[data-progress-action="open"]').click(); await flush();
    expect(onOpenWorkspace).toHaveBeenCalledWith(expect.objectContaining({ workspace: result }));
  });
  it("focuses Close while provisioning and traps Tab inside the progress dialog", () => {
    open(operation());
    const close = document.querySelector('[data-progress-action="close"]');
    expect(document.activeElement).toBe(close);
    const tab = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    close.dispatchEvent(tab);
    expect(tab.defaultPrevented).toBe(true); expect(document.activeElement).toBe(close);
  });
  it("wraps focus backwards from the first terminal action to Close", () => {
    open(failed());
    const retry = document.querySelector('[data-progress-action="retry"]'); retry.focus();
    retry.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true, cancelable: true }));
    expect(document.activeElement).toBe(document.querySelector('[data-progress-action="close"]'));
  });
  it("preserves kind, dim device identity and task board outcome in the target summary", () => {
    open(operation(), { target: { kind: "project", name: "Payments", summaryLabel: "Sources › Payments", deviceLabel: "Laptop", offline: true, taskText: "7 open tasks" } });
    const summary = document.querySelector(".create-target-summary");
    expect(summary.querySelector('.create-kind[aria-hidden="true"]')).not.toBeNull();
    expect(summary.querySelector(".dim.inbox-device").textContent).toBe("Laptop");
    expect(summary.textContent).toContain("Offline");
    expect(document.body.textContent).toContain("Uses Payments's task board · 7 open tasks");
  });
  it("shows terminal Done steps after the last running progress update", () => {
    const model = createWorkspaceProgressModel(operation({ state: "succeeded", confirmation: { result: { project, workspace } }, progress: [{ step: "project", state: "done" }, { step: "workspace", state: "running", detail: "Copying source" }] }));
    expect(model.phase).toBe("done"); expect(model.steps.map(step => step.state)).toEqual(["done", "done"]);
  });
  it("offers recovery for a terminal partial failure despite generic unresolved bookkeeping", () => {
    const row = failed(); row.unresolved = true; row.effect = "partial";
    const model = createWorkspaceProgressModel(row);
    expect(model.phase).toBe("failed"); expect(model.actions.map(([action]) => action)).toContain("retry");
  });
  it("keeps a terminal failure with unknown effect Checking without recovery actions", () => {
    const row = failed(); row.effect = "unknown"; row.unresolved = true;
    const model = createWorkspaceProgressModel(row);
    expect(model.phase).toBe("checking"); expect(model.actions).toEqual([]);
  });
  it("keeps a failure with no reliable effect evidence Checking", () => {
    const row = failed(); delete row.effect;
    expect(createWorkspaceProgressModel(row).phase).toBe("checking");
  });
  it("notifies Keep it separately from Close so the completed choice can clear its draft", async () => {
    const onKeepProject = vi.fn(); open(failed(), { onKeepProject });
    document.querySelector('[data-progress-action="keep"]').click(); await flush();
    expect(onKeepProject).toHaveBeenCalledWith(expect.objectContaining({ operationId: "attempt-1", project }));
  });
  it("does not auto-open when focus has moved elsewhere", async () => {
    vi.useFakeTimers(); const onOpenWorkspace = vi.fn(); open(completed(), { onOpenWorkspace, autoOpenDelay: 900 });
    const elsewhere = document.createElement("button"); document.body.append(elsewhere); elsewhere.focus();
    await vi.advanceTimersByTimeAsync(900); expect(onOpenWorkspace).not.toHaveBeenCalled();
  });
  it("auto-opens a completed cached attempt after a short pause", async () => {
    vi.useFakeTimers(); const onOpenWorkspace = vi.fn(); open(completed(), { onOpenWorkspace, autoOpenDelay: 900 });
    await vi.advanceTimersByTimeAsync(900); expect(onOpenWorkspace).toHaveBeenCalledTimes(1);
  });
});

it('shows a retained transport uncertainty as Checking while the worker keeps polling', () => {
 const model=createWorkspaceProgressModel({operationId:'lost',state:'queued',checking:true,request:{method:'workspace.create',params:{project_id:'p'}}});
 expect(model.phase).toBe('checking');expect(model.actions).toEqual([]);expect(model.message).toBe('Build lost the reply; checking what happened');
});

it('keeps focus in the replacement dialog when Back opens the recovered legacy form',async()=>{
  const opener=document.createElement('button');document.body.append(opener);opener.focus();
  let field;
  const onBack=()=>{const replacement=document.createElement('div');replacement.setAttribute('role','dialog');field=document.createElement('input');replacement.append(field);document.body.append(replacement);field.focus();};
  const screen=open(operation({state:'failed',effect:'unapplied',confirmation:{error:{message:'Update first'}}}),{onBack});await screen.ready;
  document.querySelector('[data-progress-action="back"]').click();await flush();
  expect(document.activeElement).toBe(field);
});

it("shows all three environment-create steps and infers partial states from landed resources", () => {
  const environment = {environment_id:"e1",name:"Sources"};
  const model=createWorkspaceProgressModel(operation({request:{method:"environment.create"},state:"failed",effect:"partial",confirmation:{error:{message:"project refused",details:{environment}}}}));
  expect(model.steps.map(({step,state})=>[step,state])).toEqual([["environment","done"],["project","failed"],["workspace","waiting"]]);
  expect(model.actions.map(([action])=>action)).toEqual(["resume","keepEnvironment","discardEnvironment"]);
  expect(model.message).toContain("no projects yet");
});
it.each([["resume","onResumeEnvironment"],["discardEnvironment","onDiscardEnvironment"]])("passes the landed environment and creating attempt to %s", async(action,handler)=>{
  const environment={environment_id:"e1",name:"Sources"}; const callback=vi.fn();
  open(operation({request:{method:"environment.create"},state:"failed",effect:"partial",confirmation:{error:{message:"refused",details:{environment}}}}),{[handler]:callback});
  document.querySelector(`[data-progress-action="${action}"]`).click(); await flush();
  expect(callback).toHaveBeenCalledWith(expect.objectContaining({environment,operationId:"attempt-1"}));
});
it("lists both landed resources when the workspace fails after environment and project creation",()=>{
 const environment={environment_id:"e1",name:"Sources"};
 const model=createWorkspaceProgressModel(operation({request:{method:"environment.create"},state:"failed",effect:"partial",confirmation:{error:{message:"checkout refused",details:{environment,project}}}}));
 expect(model.steps.map(step=>step.state)).toEqual(["done","done","failed"]);
 expect(model.message).toContain("Environment Sources was created");expect(model.message).toContain("Project Payments was created");
});
