// The C3 downloads payload, with one home. Every test in the onboarding stream
// codes against this object — the four platform keys, their labels and their
// release URLs, in the order the contract pins — so a key or a label that moves
// moves once. Tests that care about a single platform pass a `platforms`
// override; nothing else here is worth restating per file.

export const asset = (key) =>
  `https://github.com/ZechCodes/build-releases/releases/latest/download/build-bridge-${key}.tar.gz`;

export const downloadsPayload = (overrides = {}) => ({
  install_command: "curl -fsSL https://getbuild.ing/install.sh | sh",
  install_script_url: "https://getbuild.ing/install.sh",
  releases_url: "https://github.com/ZechCodes/build-releases/releases/latest",
  checksums_url: "https://github.com/ZechCodes/build-releases/releases/latest/download/SHA256SUMS",
  platforms: [
    { key: "macos-arm64", label: "macOS · Apple silicon", url: asset("macos-arm64") },
    { key: "macos-x86_64", label: "macOS · Intel", url: asset("macos-x86_64") },
    { key: "linux-x86_64", label: "Linux · x86_64", url: asset("linux-x86_64") },
    { key: "linux-aarch64", label: "Linux · arm64", url: asset("linux-aarch64") },
  ],
  ...overrides,
});
