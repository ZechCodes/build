// When a conversation compacts, as the ⋮ on its head offers it.
import { describe, expect, it, vi } from "vitest";

import {
  compactionMenuOptions,
  compactionLimitOfOptionId,
  compactionSettingsParams,
  createCompactionChoice,
} from "../src/core/conversationCompaction.js";

const labels = (agent) => compactionMenuOptions(agent).map((option) => option.label);
const marked = (agent) => compactionMenuOptions(agent).filter((option) => option.selected).map((option) => option.id);

describe("the compaction rows", () => {
  it("offers inheritance and positive presets only", () => {
    expect(labels({ max_context_tokens: null })).toEqual(["Use inherited setting", "150k", "200k", "300k"]);
    expect(marked({ max_context_tokens: 0 })).toEqual(["compact:default"]);
  });

  it("names inherited tokens and source even while an override stands", () => {
    const agent = { max_context_tokens: 300000, compaction: { inherited: { tokens: 150000, source: { kind: "workspace", id: "ws-1" } } } };
    expect(labels(agent)[0]).toBe("Use inherited setting — 150k tokens from workspace");
    expect(marked(agent)).toEqual(["compact:300000"]);
  });

  it("keeps exact custom safe integers", () => {
    expect(labels({ max_context_tokens: 150001 })).toContain("Custom (150,001 tokens)");
    expect(marked({ max_context_tokens: 150001 })).toEqual(["compact:150001"]);
    expect(compactionLimitOfOptionId("compact:150001")).toEqual({ maxContextTokens: 150001 });
  });

  it.each([2 ** 53, 2 ** 64, 1e21])("never offers an unsafe custom threshold %s", (limit) => {
    expect(compactionMenuOptions({ max_context_tokens: limit })).toHaveLength(4);
    expect(compactionLimitOfOptionId(`compact:${limit}`)).toBeNull();
  });

  it("keeps an unsafe inherited decimal exact without making it writable", () => {
    expect(labels({ compaction: { inherited: { tokens: null, tokens_decimal: "18446744073709551615", source: { kind: "device" } } } })[0])
      .toBe("Use inherited setting — 18446744073709551615 tokens from device");
  });
});

describe("reading a row back", () => {
  it("answers the limit each row sends", () => {
    expect(
      ["compact:default", "compact:150000", "compact:200000", "compact:300000"].map((id) =>
        compactionLimitOfOptionId(id),
      ),
    ).toEqual([{ maxContextTokens: null }, { maxContextTokens: 150000 }, { maxContextTokens: 200000 }, { maxContextTokens: 300000 }]);
  });

  it("answers the limit a custom row stands for", () => {
    expect(compactionLimitOfOptionId("compact:250000")).toEqual({ maxContextTokens: 250000 });
  });

  it("answers null for a row that is not one of these", () => {
    expect(compactionLimitOfOptionId("detail:all")).toBe(null);
    expect(compactionLimitOfOptionId("compact:abc")).toBe(null);
    expect(compactionLimitOfOptionId("compact:-5")).toBe(null);
    expect(compactionLimitOfOptionId("compact:off")).toBe(null);
    expect(compactionLimitOfOptionId("compact:0")).toBe(null);
    expect(compactionLimitOfOptionId("compact:1.5")).toBe(null);
    expect(compactionLimitOfOptionId("compact:0x10")).toBe(null);
    expect(compactionLimitOfOptionId("compact: 5")).toBe(null);
    expect(compactionLimitOfOptionId("compact:")).toBe(null);
    expect(compactionLimitOfOptionId("compact:Infinity")).toBe(null);
    expect(compactionLimitOfOptionId(undefined)).toBe(null);
  });

  it("asks conversation.settings in the fixture's shape", () => {
    expect(compactionSettingsParams("run-7", "agent-2", 150000)).toEqual({
      entity_id: "run-7",
      agent_id: "agent-2",
      max_context_tokens: 150000,
    });
  });
});

describe("writing what was answered", () => {
  const AGENT = { id: "agent-2", max_context_tokens: null, compact_at_tokens: 200000 };
  const SAVED = { agent_id: "agent-2", max_context_tokens: 300000, compact_at_tokens: 300000 };

  /** The row's write as a capture sees it, and a write that lays the rewrite
   *  over one held agent — the real cache's ordering is
   *  compactionAnswerCache.test.js's. */
  const cacheHolding = (agent) => {
    const cache = { agent, order: 1 };
    const steps = [];
    const capture = vi.fn(async (entityId) => {
      steps.push("capture");
      return { entityId, order: cache.order };
    });
    const write = vi.fn(async (captured, agentId, rewrite) => {
      steps.push("write");
      const next = rewrite(cache.agent);
      if (next) cache.agent = next;
    });
    const call = (answer) => vi.fn(async () => {
      steps.push("call");
      return answer;
    });
    return { cache, steps, capture, write, call };
  };

  it("takes the row's write before asking and offers the answer against it", async () => {
    const held = cacheHolding(AGENT);
    const choice = createCompactionChoice({ call: held.call(SAVED), capture: held.capture, write: held.write });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 300000 });

    expect(held.steps).toEqual(["capture", "call", "write"]);
    expect(held.write).toHaveBeenCalledWith({ entityId: "run-7", order: 1 }, "agent-2", expect.any(Function));
    expect(held.cache.agent).toEqual({ id: "agent-2", max_context_tokens: 300000, compact_at_tokens: 300000 });
  });

  it("leaves an agent that already says the answer alone", async () => {
    const pushed = { ...AGENT, max_context_tokens: 300000, compact_at_tokens: 300000 };
    const held = cacheHolding(pushed);
    const choice = createCompactionChoice({ call: held.call(SAVED), capture: held.capture, write: held.write });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 300000 });

    expect(held.cache.agent).toBe(pushed);
  });

  it("sends nothing for the row already standing, or while a choice is in flight", async () => {
    let release;
    const call = vi.fn(() => new Promise((resolve) => (release = resolve)));
    const held = cacheHolding(AGENT);
    const choice = createCompactionChoice({ call, capture: held.capture, write: held.write });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: null });
    const first = choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 150000 });
    await Promise.resolve();
    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 300000 });
    release({ max_context_tokens: 150000, compact_at_tokens: 150000 });
    await first;

    expect(call.mock.calls).toEqual([
      ["conversation.settings", { entity_id: "run-7", agent_id: "agent-2", max_context_tokens: 150000 }],
    ]);
  });

  it("hands a refusal to onFailure and writes nothing", async () => {
    const onFailure = vi.fn();
    const held = cacheHolding(AGENT);
    const choice = createCompactionChoice({
      call: async () => {
        throw new Error("not_found");
      },
      capture: held.capture,
      write: held.write,
      onFailure,
    });

    await choice.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 300000 });

    expect(onFailure).toHaveBeenCalledTimes(1);
    expect(held.write).not.toHaveBeenCalled();
    expect(held.cache.agent).toBe(AGENT);
  });

  it("does not call a cache that cannot take the answer a refusal", async () => {
    const onFailure = vi.fn();
    const failing = async () => {
      throw new Error("cache unavailable");
    };
    const unread = createCompactionChoice({ call: async () => SAVED, capture: failing, write: vi.fn(), onFailure });
    const unwritten = createCompactionChoice({
      call: async () => SAVED,
      capture: async () => ({ order: 1 }),
      write: failing,
      onFailure,
    });

    await unread.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 300000 });
    await unwritten.choose({ entityId: "run-7", agent: AGENT, maxContextTokens: 300000 });

    expect(onFailure).not.toHaveBeenCalled();
  });
});
