// These controls stay bound to their session and retain ordered final replies.
const exemptMethods = new Set([
  "session.hello", "writes.subscribe", "stream.start", "term.attach", "agent.attach",
  "term.input", "term.resize", "term.ack",
]);

function isWriteMethod(method) {
  return typeof method === "string" && /^[a-zA-Z][\w]*(?:\.[a-zA-Z][\w]*)+$/.test(method)
    && !exemptMethods.has(method) && !method.startsWith("rtc.") && !method.startsWith("changes.");
}

/** A list admits exact semantic methods only; a wildcard never grants writes. */
export function ackConfirmMethods(greeting, epoch) {
  if (!epoch || !Array.isArray(greeting?.ack_confirm_methods)) return [];
  return [...new Set(greeting.ack_confirm_methods.filter(isWriteMethod))];
}

/** Persist and submit through the shared service, resolving only acceptance. */
export async function submitWrite(writeOperations, method, params, options) {
  const operationId = params?.operation_id || options?.operationId || crypto.randomUUID();
  const result = await writeOperations.submit({ ...options, method, params, options, operationId });
  return { ...result, phase: "ack", operation_id: operationId };
}
