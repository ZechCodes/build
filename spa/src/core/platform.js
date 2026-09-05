// Which bridge build this browser should be offered, as one of the four keys
// the release matrix, install.sh and /app/downloads all speak:
// macos-arm64, macos-x86_64, linux-x86_64, linux-aarch64.
//
// The browser cannot reliably tell Apple silicon from Intel: every Mac reports
// navigator.platform "MacIntel" and a user agent carrying "Intel Mac OS X",
// Apple silicon and Rosetta included. So the word "Intel" is boilerplate, not
// evidence — the only Intel signal worth trusting is userAgentData.architecture,
// and a Mac with no architecture hint is Apple silicon.
//
// A table, walked in order, first match wins. Nothing else in the client
// branches on a platform: the downloads renderer takes the key this returns and
// looks it up in the payload's platforms list.

const shapeText = ({ platform, userAgent }) => `${platform} ${userAgent}`;

// Handhelds ship no bridge, and their user agents claim both "Mac OS X"
// (iOS) and "Linux" (Android) — so they are ruled out before either family.
const isHandheld = (shape) => /iphone|ipad|ipod|android/i.test(shapeText(shape));
const isMac = (shape) => /mac/i.test(shapeText(shape));
const isLinux = (shape) => /linux/i.test(shapeText(shape));

// "Intel Mac OS X" appears on Apple silicon too; an explicit x86_64 token does
// not, and userAgentData.architecture, when the browser offers it, is decisive.
const isMacIntel = ({ architecture, ...shape }) =>
  architecture === undefined ? /x86_64|x86-64/i.test(shapeText(shape)) : architecture === "x86";

const isArm = (shape) => /aarch64|arm64|armv8/i.test(shapeText(shape));

export const PLATFORM_MATCHERS = [
  [isHandheld, null],
  [(shape) => isMac(shape) && isMacIntel(shape), "macos-x86_64"],
  [isMac, "macos-arm64"],
  [(shape) => isLinux(shape) && isArm(shape), "linux-aarch64"],
  [isLinux, "linux-x86_64"],
];

/** The platform key for a navigator-shaped object, or null when no bridge is
 *  built for it. `architecture` is userAgentData's, absent on most browsers. */
export function platformKeyFor({ platform, userAgent, architecture } = {}) {
  const shape = { platform: platform ?? "", userAgent: userAgent ?? "", architecture };
  const matched = PLATFORM_MATCHERS.find(([test]) => test(shape));
  return matched ? matched[1] : null;
}

/** The only reader of `navigator` in the client. */
export function currentPlatformKey() {
  const data = navigator.userAgentData;
  return platformKeyFor({
    platform: data?.platform ?? navigator.platform,
    userAgent: navigator.userAgent,
    architecture: data?.architecture,
  });
}
