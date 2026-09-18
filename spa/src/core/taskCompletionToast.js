const TOAST_MS = 4000;
const EDGE_GAP = 10;

const messageFor = (titles) => {
  if (titles.length === 1) return `Task completed: ${titles[0]}`;
  const shown = titles.slice(0, 3);
  const remainder = titles.length - shown.length;
  return `${titles.length} tasks completed: ${shown.join(", ")}${remainder ? `, and ${remainder} more` : ""}`;
};

export function mountTaskCompletionToast(host, { duration = TOAST_MS } = {}) {
  let toast = null;
  let anchor = null;
  let timer = null;
  let disposed = false;
  const queue = [];

  const remove = () => {
    clearTimeout(timer);
    timer = null;
    toast?.remove();
    toast = null;
    anchor = null;
  };

  const abandonCurrent = () => {
    remove();
    showNext();
  };

  const showNext = () => {
    if (disposed || toast || !queue.length) return;
    const { agentId, titles } = queue.shift();
    const bubble = [...host.querySelectorAll('[data-bubble="agent"]')]
      .find((element) => element.dataset.agent === agentId);
    if (!bubble) return showNext();
    anchor = bubble;
    toast = document.createElement("div");
    toast.className = "task-completion-toast";
    toast.setAttribute("role", "status");
    toast.setAttribute("aria-live", "polite");
    toast.textContent = messageFor(titles);
    document.body.append(toast);
    timer = setTimeout(() => {
      remove();
      showNext();
    }, duration);
    place();
  };

  const place = () => {
    if (!toast) return;
    if (!anchor?.isConnected) return abandonCurrent();
    const bubble = anchor.getBoundingClientRect();
    const offscreen = bubble.right <= 0 || bubble.left >= window.innerWidth
      || bubble.bottom <= 0 || bubble.top >= window.innerHeight;
    if (offscreen) return abandonCurrent();
    const box = toast.getBoundingClientRect();
    const across = window.innerWidth < 761;
    const idealLeft = across ? bubble.left + bubble.width / 2 - box.width / 2 : bubble.left - box.width - EDGE_GAP;
    const idealTop = across ? bubble.top - box.height - EDGE_GAP : bubble.top + bubble.height / 2 - box.height / 2;
    const maxLeft = Math.max(EDGE_GAP, window.innerWidth - box.width - EDGE_GAP);
    const maxTop = Math.max(EDGE_GAP, window.innerHeight - box.height - EDGE_GAP);
    toast.style.left = `${Math.min(Math.max(EDGE_GAP, idealLeft), maxLeft)}px`;
    toast.style.top = `${Math.min(Math.max(EDGE_GAP, idealTop), maxTop)}px`;
  };

  window.addEventListener("resize", place);
  window.addEventListener("scroll", place, true);
  return {
    show(agentId, titles) {
      if (disposed || !titles.length) return;
      queue.push({ agentId, titles });
      showNext();
    },
    dispose() {
      disposed = true;
      queue.length = 0;
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
      remove();
    },
  };
}
