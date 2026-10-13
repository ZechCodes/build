import { expect, it, vi } from "vitest";
import { TerminalSocketLost } from "../src/terminal/session.js";
import { attachUserTerminal } from "../src/terminal/userAttachment.js";

it("keeps the same pane callbacks through a lost terminal channel and recovery", async () => {
  const options = { onSnapshot: vi.fn(), isCurrent: () => true };
  const manager = { attachTerminal: vi.fn()
    .mockRejectedValueOnce(new TerminalSocketLost("disconnected"))
    .mockResolvedValueOnce({ cursor: 2 }) };
  await expect(attachUserTerminal(manager, "shell", { project_id: "p" }, options)).resolves.toEqual({ cursor: 2 });
  expect(manager.attachTerminal.mock.calls).toEqual([
    ["shell", { project_id: "p" }, options], ["shell", { project_id: "p" }, options],
  ]);
  for (const error of [new TerminalSocketLost("closed"), new Error("unknown term_id")]) {
    const manager = { attachTerminal: vi.fn().mockRejectedValue(error) };
    await expect(attachUserTerminal(manager, "shell", {}, {})).rejects.toBe(error);
    expect(manager.attachTerminal).toHaveBeenCalledOnce();
  }
});

it("cannot retry a lost attach after the pane lifetime ends", async () => {
  let current = true;
  const manager = { attachTerminal: vi.fn(async () => {
    current = false;
    throw new TerminalSocketLost("disconnected");
  }) };
  await expect(attachUserTerminal(manager, "shell", {}, { isCurrent: () => current })).rejects.toMatchObject({ name: "AbortError" });
  expect(manager.attachTerminal).toHaveBeenCalledOnce();
});
