/** Keep a CSS length in step with an element whose controls can wrap or grow. */
export function mountMeasuredHeight(element, container, property) {
  const clear = () => container?.style.removeProperty(property);
  if (!element || !container || typeof ResizeObserver === "undefined") return clear;
  const sync = () => container.style.setProperty(property, `${element.getBoundingClientRect().height}px`);
  const observer = new ResizeObserver(sync);
  observer.observe(element);
  sync();
  return () => {
    observer.disconnect();
    clear();
  };
}
