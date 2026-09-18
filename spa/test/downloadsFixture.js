// Public download contract shared by the onboarding and settings tests.
export const asset = (key) => `https://getbuild.ing/app/downloads/${key}`;
export const installCommand = () => 'curl -fsSL "https://getbuild.ing/install.sh" | sh';
export const mintedCommand = installCommand;

export const downloadsPayload = (overrides = {}) => ({
  install_command: installCommand(),
  install_script_url: "https://getbuild.ing/install.sh",
  releases_url: "https://github.com/ZechCodes/build-releases/releases",
  desktop_install_command: 'curl -fsSL "https://getbuild.ing/install-desktop.sh" | sh',
  desktop_install_script_url: "https://getbuild.ing/install-desktop.sh",
  desktop_releases_url: "https://github.com/ZechCodes/build-releases/releases",
  checksums_url: "https://getbuild.ing/app/downloads/SHA256SUMS",
  platforms: [
    { key: "macos-arm64", label: "macOS · Apple silicon", url: asset("macos-arm64") },
    { key: "macos-x86_64", label: "macOS · Intel", url: asset("macos-x86_64") },
    { key: "linux-x86_64", label: "Linux · x86_64", url: asset("linux-x86_64") },
    { key: "linux-aarch64", label: "Linux · arm64", url: asset("linux-aarch64") },
  ],
  ...overrides,
});
