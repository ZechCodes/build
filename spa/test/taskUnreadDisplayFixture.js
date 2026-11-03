import { afterEach } from "vitest";

const owners = [];
afterEach(() => { for (const owner of owners.splice(0)) owner.dispose(); });

// Use the same selection and successful-paint hooks as a mounted task row.
// No note is persisted: these cache tests exercise the mounted display owner.
export async function rememberTaskUnreadDisplay(scope, task) {
  const { createTaskUnreadDisplayOwner } = await import("../src/core/taskUnreadDisplay.js");
  const { taskUnreadCount } = await import("../src/core/taskUnread.js");
  const owner = createTaskUnreadDisplayOwner({ scope, consumer: "tasks-pane" });
  owners.push(owner);
  await owner.updateTasks([task]);
  owner.rendered(task, null, taskUnreadCount(task));
  return (next) => owner.preview(next, null);
}
