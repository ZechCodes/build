// Mirrors #972's bridge predicates and regression cases in
// store/task_summaries/query_projection.rs and tests/task_summaries/query_attention.rs.
export const reviewServiceAttentionCases = [
  { name: "finished service ask", status: "done", reviewStatus: "rejected", serviceMentions: 1, expectedAsked: true },
  { name: "read finished ask", status: "done", reviewStatus: "rejected", serviceMentions: 0, expectedAsked: false },
  { name: "changes with author", status: "in_review", reviewStatus: "changes_requested", expectedAsked: false },
  { name: "approved with author", status: "in_review", reviewStatus: "approved", expectedAsked: false },
  { name: "open revision with user", status: "in_review", reviewStatus: "open", expectedAsked: true },
  { name: "ordinary user task", status: "ready", expectedAsked: true },
];
