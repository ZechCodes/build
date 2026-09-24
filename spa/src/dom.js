// Tiny DOM helpers used everywhere.

export const $ = (selector, root = document) => root.querySelector(selector);

export const el = (html) => {
  const template = document.createElement("template");
  template.innerHTML = html.trim();
  return template.content.firstChild;
};

// Writes that happen only when they change something. Setting an attribute to
// the value it already holds is still a mutation, and it invalidates style
// under the node. #138 measured about fifty of them a second around a focused
// text box while an agent worked, as phones dropped gesture typing in it.
// Surfaces that re-sync on every push write through these.

export const setAttr = (node, name, value) => {
  const wanted = String(value);
  if (node.getAttribute(name) !== wanted) node.setAttribute(name, wanted);
};

export const setHidden = (node, hidden) => {
  if (node.hidden !== hidden) node.hidden = hidden;
};

export const setData = (node, key, value) => {
  const wanted = String(value);
  if (node.dataset[key] !== wanted) node.dataset[key] = wanted;
};
