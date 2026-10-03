import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { threadWindow, syncThreadWindow } from "../src/core/threadSync.js";
import { widenCachedThread } from "../src/core/agentRailContext.js";
import { createChatRepository } from "../src/core/chatRepository.js";
import { applyConversationReset, reconcileConversationGeneration } from "../src/core/conversationReset.js";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { createCompactionChoice } from "../src/core/conversationCompaction.js";
import { createWatchToggle } from "../src/core/watchToggle.js";
import { insertRecord } from "../src/core/optimistic.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const address = { deviceId: "reset-device", entityId: "run-1", kind: "thread", sub: "conversation-1" };
const item = (sequence, body) => ({ type: "message", data: { sequence, id: `m${sequence}`, role: "user", body } });
const held = { thread_id: "new-thread", items: [item(4, "fresh words")], deliveredSequence: 4, activityDigests: [] };
beforeEach(async () => { await wipeCache(); });

describe("a cleared conversation's generation", () => {
  it("rejects a page from the previous thread even when its sequence is larger", () => {
    expect(threadWindow(held, { thread_id: "old-thread", items: [item(50, "old words")] })).toBe(null);
  });
  it("keeps the fresh generation on empty pages", () => {
    const empty = threadWindow(null, { thread_id: "new-thread", items: [], thread_total: 0 });
    expect(empty).toMatchObject({ thread_id: "new-thread", items: [], deliveredSequence: 0 });
  });
  it("rejects an old older-page answer after the replacement reaches the same sequence", async () => {
    await writeCached(address, held);
    await widenCachedThread(address, { thread_id: "old-thread", items: [item(3, "old words")] }, 4);
    expect((await readCached(address)).value.items).toEqual(held.items);
  });
  it("retires old drafts and pending submissions when another client changes the generation", async () => {
    const repo = createChatRepository({ scope: { deviceId: "reset-device" }, call: async () => ({}), storage: null });
    const identity = { entityId: "run-1", agentId: "agent-1", conversationId: "conversation-1", threadId: "old-thread" };
    const old = repo.controller(identity);
    old.writeDraft({ body: "old draft" });
    const submission = old.captureSubmission();
    const fresh = repo.controller({ ...identity, threadId: "new-thread" });
    expect(fresh).not.toBe(old);
    expect(fresh.readDraft().body).toBe("");
    expect(() => repo.assertSubmissionActive(submission)).toThrow(/cleared|generation/);
    repo.dispose();
  });
});

it("a shared cache reset retires another client's offscreen draft, attachments and queued send", async () => {
  const first = createChatRepository({ scope: { deviceId: "reset-device" }, call: async () => ({}), storage: null });
  const other = createChatRepository({ scope: { deviceId: "reset-device" }, call: async () => ({}), storage: null });
  const identity = { entityId: "run-1", agentId: "agent-1", conversationId: "conversation-1", threadId: "old-thread" };
  const old = other.controller(identity);
  old.threadState.rememberAttachment("old.png", { body: "secret" });
  old.writeDraft({ body: "secret draft" });
  const submission = old.captureSubmission();
  await writeCached(address, { thread_id: "new-thread", items: [], deliveredSequence: 0, retired_thread_ids: ["old-thread"] });
  await vi.waitFor(() => expect(() => other.assertSubmissionActive(submission)).toThrow(/cleared|generation/));
  expect(old.threadState.attachment("old.png")).toBe(undefined);
  expect(old.readDraft().body).toBe("");
  expect(other.controller({ ...identity, threadId: "new-thread" }).readDraft().body).toBe("");
  first.dispose(); other.dispose();
});

it("orders resets by canonical generation even when aliases have incomparable model choice revisions", async () => {
  await writeCached(address, { ...held, thread_id: "generation-8", thread_generation_revision: 8 });
  await reconcileConversationGeneration({ deviceId: address.deviceId, entityId: address.entityId,
    agent: { id: "alias-b", conversation_id: address.sub, thread_id: "generation-9", thread_generation_revision: 9, choice_revision: 2 } });
  expect((await readCached(address)).value.thread_id).toBe("generation-9");
  await reconcileConversationGeneration({ deviceId: address.deviceId, entityId: address.entityId,
    agent: { id: "alias-a", conversation_id: address.sub, thread_id: "generation-8", thread_generation_revision: 8, choice_revision: 100 } });
  expect((await readCached(address)).value.thread_id).toBe("generation-9");
});
it("drops legacy content when an offline browser learns a reset generation", async () => {
  await writeCached(address, { items: [item(100, "forgotten legacy words")], deliveredSequence: 100 });
  await reconcileConversationGeneration({ deviceId: address.deviceId, entityId: address.entityId,
    agent: { id: "agent-1", conversation_id: address.sub, thread_id: "thread:conversation-1:reset-uuid", thread_generation_revision: 1 } });
  expect((await readCached(address)).value.items).toEqual([]);
});
it("an in-flight old forward page cannot append words or repair debt after reset", async () => {
  await writeCached(address, { thread_id: "old-thread", items: [item(99, "old words")], deliveredSequence: 99 });
  let answer;
  let asked = false;
  const syncing = syncThreadWindow({ deviceId: address.deviceId, entityId: address.entityId, agentId: "agent-1", conversationId: address.sub,
    call: async (_method, params) => { asked = true; expect(params.thread_id).toBe("old-thread"); return new Promise((resolve) => { answer = resolve; }); } });
  await vi.waitFor(() => expect(asked).toBe(true));
  await writeCached(address, { ...held, thread_id: "new-thread", thread_generation_revision: 1 });
  answer({ thread_id: "old-thread", items: [], thread_last_sequence: 200 });
  await syncing;
  expect((await readCached(address)).value).toEqual({ ...held, thread_id: "new-thread", thread_generation_revision: 1 });
});

const rowAddress = { ...address, kind: "row", sub: "" };
const context = { deviceId: address.deviceId, active: () => true };
const resetAnswer = (over = {}) => ({ entity_id: address.entityId, agent_id: "agent-1", conversation_id: address.sub,
  previous_thread_id: "old-thread", thread_id: "new-thread", thread_generation_revision: 1,
  agent: { id: "agent-1", conversation_id: address.sub, thread_id: "new-thread", thread_generation_revision: 1 },
  thread: { thread_id: "new-thread", items: [], sessions: [] }, ...over });

it("a late earlier reset answer cannot replace the current agent or primary summary", async () => {
  const answer = resetAnswer();
  const row = { agents: [{ ...answer.agent, thread_id: "newer-thread", thread_generation_revision: 2 }], summary: "fresh summary" };
  await writeCached(address, { thread_id: "newer-thread", thread_generation_revision: 2, items: [] });
  await writeCached(rowAddress, row);
  await applyConversationReset(context, answer);
  expect((await readCached(rowAddress)).value).toEqual(row);
});

it("preserves the primary summary and thread when clearing a separate secondary conversation", async () => {
  const primary = { id: "primary", conversation_id: "primary-conversation", thread_id: "primary-thread" };
  const secondary = { id: "agent-1", conversation_id: address.sub, thread_id: "old-thread" };
  const row = { summary: "primary words", last_error: "primary failure", agents: [primary, secondary], thread: { id: "primary-thread", items: [item(2, "primary words")] } };
  await writeCached(rowAddress, row);
  await applyConversationReset(context, resetAnswer());
  expect((await readCached(rowAddress)).value).toEqual({ ...row, agents: [primary, resetAnswer().agent] });
});

it("clears the primary summary and nested transcript when an alias clears its canonical conversation", async () => {
  const primary = { id: "primary", conversation_id: address.sub, thread_id: "old-thread" };
  const row = { summary: "old summary", last_error: "old error", agents: [primary, { ...primary, id: "agent-1" }],
    run: { summary: "old summary", agents: [primary], thread: { id: "old-thread", items: [item(2, "old words")] } } };
  await writeCached(rowAddress, row);
  await applyConversationReset(context, resetAnswer());
  expect((await readCached(rowAddress)).value).toMatchObject({ summary: "", last_error: null, run: { summary: "", thread: resetAnswer().thread } });
});

it("purges cached legacy activity and surfaces for every canonical alias while retaining a sibling", async () => {
  const agent = { id: "agent-1", conversation_id: address.sub, thread_id: "old-thread" };
  await writeCached(rowAddress, { agents: [agent, { ...agent, id: "alias-2" }, { id: "sibling", conversation_id: "other-conversation", thread_id: "other-thread" }] });
  const replicas = ["agent-1", "alias-2", "sibling"].flatMap((id) => [
    { ...address, kind: "surfaces", sub: id }, { ...address, kind: "activity", sub: `${id}:run-1` },
  ]);
  for (const replica of replicas) await writeCached(replica, { body: "stored words" });
  await applyConversationReset(context, resetAnswer());
  for (const replica of replicas) expect(Boolean(await readCached(replica))).toBe(replica.sub.startsWith("sibling"));
});

it("immediately replaces cached alias generations and summaries on another owner", async () => {
  const otherRowAddress = { ...rowAddress, entityId: "other-owner" };
  const otherThreadAddress = { ...address, entityId: "other-owner" };
  const alias = { id: "alias-2", conversation_id: address.sub, thread_id: "old-thread", thread_generation_revision: 0,
    provider: "pi", model: "alias-model", choice_revision: 5, active_model: "old-active-model", topic: "old topic", unread_count: 8, working: true, last_context_tokens: 40000 };
  await writeCached(otherRowAddress, { agents: [alias], summary: "old summary", thread: { id: "old-thread", items: [item(10, "other owner old words")] } });
  await writeCached(otherThreadAddress, { thread_id: "old-thread", items: [item(10, "other owner old words")], deliveredSequence: 10 });
  await applyConversationReset(context, resetAnswer());
  const otherRow = (await readCached(otherRowAddress)).value;
  expect(otherRow.agents[0]).toMatchObject({ provider: "pi", model: "alias-model", thread_id: "new-thread", thread_generation_revision: 1,
    choice_revision: 6, active_model: "alias-model", topic: null, unread_count: 0, working: false, last_context_tokens: null });
  expect(otherRow.thread.items).toEqual([]);
  expect(otherRow.summary).toBe("");
  expect((await readCached(otherThreadAddress)).value.items).toEqual([]);
});

it("keeps model, watching and compaction mutations tied to the fresh generation", async () => {
  const call = vi.fn(async () => ({}));
  const repo = createChatRepository({ scope: { deviceId: "reset-device" }, call, storage: null });
  const fresh = repo.controller({ entityId: address.entityId, agentId: "agent-1", conversationId: address.sub, threadId: "fresh-thread" });
  await fresh.chooseModel({ model: "next-model", effort: "high" });
  await createWatchToggle({ entityId: address.entityId, agentId: "agent-1", threadId: "fresh-thread", call }).press();
  await createCompactionChoice({ call, capture: async () => null, write: async () => {} }).choose({
    entityId: address.entityId, agent: { id: "agent-1", thread_id: "fresh-thread" }, maxContextTokens: 150000,
  });
  expect(call.mock.calls.map(([method, params]) => [method, params.thread_id])).toEqual([
    ["agent.choose", "fresh-thread"], ["conversation.watch", "fresh-thread"], ["conversation.settings", "fresh-thread"],
  ]);
  repo.dispose();
});

it("retires a first-message optimistic projection and refuses its delayed scope migration", async () => {
  const repo = createChatRepository({ scope: { deviceId: "reset-device" }, call: async () => ({}), storage: null });
  const identity = { entityId: address.entityId, agentId: "agent-1", conversationId: address.sub, threadId: "old-thread" };
  repo.controller(identity);
  const store = repo.optimisticStore();
  const oldScope = `${repo.scopeKey}:thread:rail:agent-1:old-thread`;
  const freshScope = `${repo.scopeKey}:thread:rail:agent-1:fresh-thread`;
  let finish;
  const waiting = new Promise((resolve) => { finish = resolve; });
  const settling = store.runOptimistic({ scope: oldScope, records: [insertRecord("first-message", { body: "old optimistic words" })],
    call: async (handle) => { await waiting; handle.moveScope(oldScope, freshScope); } });
  expect(store.pendingIn(oldScope)).toHaveLength(1);
  repo.controller({ ...identity, threadId: "fresh-thread", threadGenerationRevision: 1 });
  expect(store.pendingIn(oldScope)).toEqual([]);
  finish(); await settling;
  expect(store.pendingIn(freshScope)).toEqual([]);
  repo.dispose();
});
