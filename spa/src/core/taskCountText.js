/** Shared project task count wording for environment and workspace consumers. */
export const taskCountText = (open, total) => open > 0 ? `${open} open task${open===1 ? "":"s"}` : `${total} task${total===1 ? "":"s"}`;
