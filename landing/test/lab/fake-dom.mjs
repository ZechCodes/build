// A document small enough for the wall to draw into: elements that record
// their children, classes, data and inline style, and animate as
// fake-animation.mjs's do.
import { element } from "../hero/fake-animation.mjs";

export function fakeDocument() {
  const document = {
    createElement(tag) {
      const made = element();
      const properties = {};
      return Object.assign(made, {
        tag,
        className: "",
        dataset: {},
        textContent: "",
        children: [],
        parent: null,
        ownerDocument: document,
        style: { ...made.style, setProperty: (name, value) => { properties[name] = value; }, properties },
        append(...children) {
          for (const child of children) {
            child.parent = this;
            this.children.push(child);
          }
        },
        remove() {
          if (this.parent) this.parent.children = this.parent.children.filter((child) => child !== this);
          this.parent = null;
        },
        all() { return this.children.flatMap((child) => [child, ...child.all()]); },
      });
    },
  };
  return document;
}
