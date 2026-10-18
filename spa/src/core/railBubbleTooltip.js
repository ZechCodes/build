import { workingClock, workingSeconds } from "./agentRailModel.js";

const EDGE = 8;
const GAP = 8;
let nextTooltipId = 0;

const clamp = (value, lower, upper) => Math.max(lower, Math.min(value, Math.max(lower, upper)));
const focusVisible = (button) => !!button && button.isConnected
  && document.activeElement === button && button.matches(":focus-visible");

/** The tip lives on the page, outside the rail's scrolling strip. */
export function positionRailBubbleTooltip(tip, button) {
  const anchor = button.getBoundingClientRect();
  const bounds = tip.getBoundingClientRect();
  const width = window.innerWidth;
  const height = window.innerHeight;
  const beside = anchor.left - bounds.width - GAP;
  const left = beside >= EDGE ? beside : anchor.right + GAP;
  const top = anchor.top + (anchor.height - bounds.height) / 2;
  tip.style.left = `${clamp(left, EDGE, width - bounds.width - EDGE)}px`;
  tip.style.top = `${clamp(top, EDGE, height - bounds.height - EDGE)}px`;
}

const tooltipStatus = (facts) => {
  if (facts.status === "starting") return "Starting";
  if (facts.status !== "working") return "Idle";
  const elapsed = workingSeconds(facts.workingTime, Date.now());
  return elapsed === null ? "Working" : `Working · ${workingClock(elapsed)}`;
};

const tooltipLine = (tip, className, content) => {
  const line = document.createElement("div");
  line.className = className;
  line.textContent = content;
  tip.append(line);
};

function paintTooltip(tip, facts) {
  tip.replaceChildren();
  tooltipLine(tip, "rail-bubble-tooltip-name", facts.name || "Agent");
  tooltipLine(tip, "rail-bubble-tooltip-work", facts.work || facts.description || "No current work");
  tooltipLine(tip, "rail-bubble-tooltip-status", tooltipStatus(facts));
  const details = [facts.purpose, [facts.provider, facts.model].filter(Boolean).join(" · "), facts.watch, facts.descendants].filter(Boolean);
  for (const detail of details) tooltipLine(tip, "rail-bubble-tooltip-detail", detail);
  if (facts.news && facts.news !== "working") tooltipLine(tip, "rail-bubble-tooltip-news", facts.news);
}

/** One mounted rail owns one tooltip; kept bubble buttons read their current
 * dataset on every show/refresh, so a cache repaint never leaves stale facts. */
export function createRailBubbleTooltip() {
  let active = null;
  let tip = null;
  let interval = null;
  let disposed = false;
  let hovered = null;
  let focused = null;
  const tooltipId = `rail-bubble-tooltip-${++nextTooltipId}`;
  const onEscape = (event) => { if (event.key === "Escape") hide(); };
  const hide = () => {
    active?.removeAttribute("aria-describedby");
    active = null;
    tip?.remove();
    tip = null;
    if (interval !== null) clearInterval(interval);
    interval = null;
    window.removeEventListener("resize", refresh);
    document.removeEventListener("scroll", refresh, true);
    document.removeEventListener("keydown", onEscape);
  };
  const refresh = () => {
    if (!active) return;
    if (!active.isConnected || !active.dataset.railTooltip) {
      const old = active;
      hide();
      restore(old);
      return;
    }
    paintTooltip(tip, JSON.parse(active.dataset.railTooltip));
    active.setAttribute("aria-describedby", tooltipId);
    positionRailBubbleTooltip(tip, active);
  };
  const show = (button) => {
    if (disposed || !button.isConnected || !button.dataset.railTooltip) return;
    if (active !== button) hide();
    active = button;
    if (!tip) {
      tip = document.createElement("div");
      tip.className = "rail-bubble-tooltip";
      tip.id = tooltipId;
      tip.setAttribute("role", "tooltip");
      document.body.append(tip);
      interval = setInterval(refresh, 1000);
      window.addEventListener("resize", refresh);
      document.addEventListener("scroll", refresh, true);
      document.addEventListener("keydown", onEscape);
    }
    button.setAttribute("aria-describedby", tip.id);
    refresh();
  };
  const restore = (leaving) => {
    if (focused && !focused.isConnected) focused = null;
    if (hovered && !hovered.isConnected) hovered = null;
    if (focused !== leaving && focusVisible(focused)) show(focused);
    else if (hovered && hovered !== leaving) show(hovered);
  };
  return {
    wire(button) {
      let touchOrigin = false;
      button.addEventListener("pointerdown", (event) => {
        touchOrigin = event.pointerType === "touch";
        if (touchOrigin) {
          if (hovered === button) hovered = null;
          if (active === button) hide();
        }
      });
      button.addEventListener("pointerenter", (event) => {
        if (event.pointerType === "touch") return;
        hovered = button;
        show(button);
      });
      button.addEventListener("pointerleave", () => {
        if (hovered === button) hovered = null;
        if (active === button) { hide(); restore(button); }
      });
      button.addEventListener("click", () => {
        if (hovered === button) hovered = null;
        if (focused === button) focused = null;
        if (active === button) hide();
      });
      button.addEventListener("focus", () => {
        if (!touchOrigin && button.matches(":focus-visible")) { focused = button; show(button); }
      });
      button.addEventListener("keydown", (event) => {
        if (event.key === "Escape") return;
        touchOrigin = false;
        if (document.activeElement === button && button.matches(":focus-visible")) { focused = button; show(button); }
      });
      button.addEventListener("blur", () => {
        touchOrigin = false;
        if (focused === button) focused = null;
        if (active === button && hovered !== button) { hide(); restore(button); }
      });
    },
    refresh,
    dispose() { disposed = true; hovered = null; focused = null; hide(); },
  };
}
