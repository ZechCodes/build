export const APP_URL = "https://getbuild.ing/app/desktop";

const { origin: APP_ORIGIN, pathname: APP_PATH } = new URL(APP_URL);
const EXTERNAL_PROTOCOLS = new Set(["http:", "https:", "mailto:"]);

function parseUrl(value) {
  try {
    return new URL(value);
  } catch {
    return null;
  }
}

export function classifyNavigation(value) {
  const url = parseUrl(value);
  if (!url) return "deny";
  if (url.origin === APP_ORIGIN && url.pathname === APP_PATH) return "internal";
  if (EXTERNAL_PROTOCOLS.has(url.protocol)) return "external";
  return "deny";
}

export function installNavigationPolicy(webContents, { openExternal, loadInternal }) {
  const handleNavigation = (event, url) => {
    const destination = classifyNavigation(url);
    if (destination === "internal") return;

    event.preventDefault();
    if (destination === "external") openExternal(url);
  };

  webContents.on("will-navigate", handleNavigation);
  webContents.on("will-redirect", handleNavigation);
  webContents.on("will-attach-webview", (event) => event.preventDefault());
  webContents.setWindowOpenHandler(({ url }) => {
    const destination = classifyNavigation(url);
    if (destination === "internal") loadInternal(url);
    if (destination === "external") openExternal(url);
    return { action: "deny" };
  });
}

export function createPermissionPolicy() {
  return (permission, requestingUrl) => {
    if (permission !== "notifications") return false;
    return parseUrl(requestingUrl)?.origin === APP_ORIGIN;
  };
}

function isAuthenticatedRequest(value, resourceType) {
  const url = parseUrl(value);
  if (url?.origin !== APP_ORIGIN) return false;
  if (url.pathname === APP_PATH) return resourceType === "mainFrame";
  // API fetches need credentials, but navigating to an API URL does not.
  return resourceType === "xhr" && url.pathname.startsWith("/api/");
}

export function authorizeRequestHeaders(url, headers, accessToken, resourceType) {
  // A redirect may carry the previous request's Authorization header.
  const cleanHeaders = Object.fromEntries(
    Object.entries(headers).filter(([name]) => name.toLowerCase() !== "authorization"),
  );
  if (!accessToken || !isAuthenticatedRequest(url, resourceType)) return cleanHeaders;
  return { ...cleanHeaders, Authorization: `Bearer ${accessToken}` };
}

export function createWindowOptions({ development }) {
  return {
    width: 1280,
    height: 800,
    minWidth: 600,
    minHeight: 500,
    backgroundColor: "#07151b",
    show: false,
    webPreferences: {
      allowRunningInsecureContent: false,
      backgroundThrottling: false,
      contextIsolation: true,
      devTools: development,
      navigateOnDragDrop: false,
      nodeIntegration: false,
      nodeIntegrationInWorker: false,
      sandbox: true,
      webSecurity: true,
      webviewTag: false,
    },
  };
}
