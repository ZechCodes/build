// Tiny DOM helpers used everywhere.

export const $ = (selector, root = document) => root.querySelector(selector);

export const el = (html) => {
  const template = document.createElement("template");
  template.innerHTML = html.trim();
  return template.content.firstChild;
};
