// The C3 downloads payload, with one home. Every test in the onboarding stream
// codes against this object — the four platform keys, their labels and their
// api urls, in the order the contract pins — so a key or a label that moves
// moves once. Tests that care about a single platform pass a `platforms`
// override; nothing else here is worth restating per file.
//
// Every download now comes from the api's own origin (the release repository is
// private, so the api streams the asset behind the alpha gate) and the install
// one-liner carries a freshly minted, short-lived download token. The two
// tokens below are the shape the api mints — "dl_" and 32 urlsafe characters —
// and are worth exactly nothing: the api is the only judge of a token.

export const asset = (key) => `https://getbuild.ing/app/downloads/${key}`;

export const installCommand = (token) => `curl -fsSL "https://getbuild.ing/install.sh?t=${token}" | sh`;

const FETCHED_TOKEN = "dl_F3n7Qk2XbVpL9sYtRmZc4WgHdJ0aUeN1"; // gitleaks:allow — typed by hand for these specs
const MINTED_TOKEN = "dl_K8pR2vMzXq4TnBwLc7YhJ0sEdF1gUaQ5"; // gitleaks:allow — typed by hand for these specs

/** The one-liner POST /app/downloads/token answers with — a second token, so a
 *  re-mint spec can tell the fresh line from the painted one. */
export const mintedCommand = () => installCommand(MINTED_TOKEN);

export const downloadsPayload = (overrides = {}) => ({
  install_command: installCommand(FETCHED_TOKEN),
  install_script_url: "https://getbuild.ing/install.sh",
  releases_url: "https://github.com/ZechCodes/build-web/releases/latest",
  checksums_url: "https://getbuild.ing/app/downloads/SHA256SUMS",
  platforms: [
    { key: "macos-arm64", label: "macOS · Apple silicon", url: asset("macos-arm64") },
    { key: "macos-x86_64", label: "macOS · Intel", url: asset("macos-x86_64") },
    { key: "linux-x86_64", label: "Linux · x86_64", url: asset("linux-x86_64") },
    { key: "linux-aarch64", label: "Linux · arm64", url: asset("linux-aarch64") },
  ],
  ...overrides,
});
