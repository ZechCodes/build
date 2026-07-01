// Pure text helpers shared by every surface.

export const esc = (value) =>
  (value ?? "").toString().replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
