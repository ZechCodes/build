// The approve link `build-bridge pair` prints: /app/#/pair/<code> (#319).
//
// The code rides in the fragment, which no request carries, so it reaches no
// server log. The page takes it out of the address before the router reads the
// hash, leaving the devices page in its place, and hands it to whoever opens
// the approve screen. The link only fills the code in: approving is still the
// reader's press, after comparing the fingerprint.

const PAIR_LINK = /^#\/pair\/([A-Za-z0-9-]{1,32})$/;
const LANDING_HASH = "#/account/devices";

/** The pairing code in an approve link's fragment, or null for any other. */
export function pairCodeFromHash(hash) {
  const match = PAIR_LINK.exec(hash || "");
  return match ? match[1].toUpperCase() : null;
}

function takePairLink(win, open) {
  const code = pairCodeFromHash(win.location.hash);
  if (!code) return;
  win.history.replaceState(win.history.state, "", `${win.location.pathname}${win.location.search}${LANDING_HASH}`);
  open(code);
}

/** Hand `open` the code of the link this page opened on, and of any followed
 *  while it is open. Install before the router: the hashchange listener added
 *  first runs first, so the router only ever reads the devices page. Answers
 *  the function that stops listening. */
export function watchPairLinks(win, open) {
  const taken = () => takePairLink(win, open);
  taken();
  win.addEventListener("hashchange", taken);
  return () => win.removeEventListener("hashchange", taken);
}
