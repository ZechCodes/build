import { expect, it, vi } from "vitest";
import { createSecretMetadataBinding } from "../src/core/secretMetadataBinding.js";

const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};

it("does not publish an old conversation read after switching bindings mid-read", async () => {
  const first = deferred();
  const second = deferred();
  const cache = {
    read: vi.fn((scope) => scope.agentId === "agent-old" ? first.promise : second.promise),
    subscribe: vi.fn(() => vi.fn()),
  };
  const publish = vi.fn();
  const binding = createSecretMetadataBinding({ cache, onPublish: publish });

  binding.bind([{ projectId: "project-1", agentId: "agent-old" }]);
  binding.bind([{ projectId: "project-1", agentId: "agent-new" }]);
  second.resolve([{ secret_id: "secret-new", revision: 1 }]);
  await vi.waitFor(() => expect(publish).toHaveBeenCalledWith([{ secret_id: "secret-new", revision: 1 }]));

  first.resolve([{ secret_id: "secret-old", revision: 9 }]);
  await Promise.resolve();
  await Promise.resolve();
  expect(publish).toHaveBeenCalledTimes(1);
});
