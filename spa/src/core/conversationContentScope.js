// Content retained in memory belongs to one device and transcript generation.
const FIELDS = ["deviceId", "entityId", "conversationId", "threadId"];
export const conversationContentKey = (scope) => FIELDS.map((field) => scope[field] || "").join("|");

// Older unscoped content is conservatively forgotten when ownership is unknown.
export const matchesConversationContent = (held, wanted) =>
  FIELDS.every((field) => !wanted[field] || !held[field] || held[field] === wanted[field]);
