// The pane is a column of siblings, so the primitive goes on a wrapper: the
// tab body caps nothing, and a pane that leaves its own width unstated there
// runs the whole viewport wide.
export function planReviewSkeletonHtml() {
  return `<div class="pane-col">
    <p class="mono projmeta" id="planmeta"></p>
    <div class="plan-summary" id="plansummary" hidden></div>
    <div id="planbody"><div class="plan-loading">loading…</div></div>
  </div>`;
}
