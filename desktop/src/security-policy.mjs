export const APP_URL = "https://getbuild.ing/app/desktop";

const APP_ORIGIN = new URL(APP_URL).origin;
const EXTERNAL_PROTOCOLS = new Set(["http:", "https:", "mailto:"]);

export function classifyNavigation(value) {
  let url;
  try {
    url = new URL(value);
  } catch {
    return "deny";
  }

  if (url.origin === APP_ORIGIN) return "internal";
  if (EXTERNAL_PROTOCOLS.has(url.protocol)) return "external";
  return "deny";
}

export function createPermissionPolicy() {
  return (permission, requestingUrl) => {
    if (permission !== "notifications") return false;
    try {
      return new URL(requestingUrl).origin === APP_ORIGIN;
    } catch {
      return false;
    }
  };
}

export function authorizeRequestHeaders(url, headers, accessToken) {
  if (!accessToken || classifyNavigation(url) !== "internal") return headers;
  return { ...headers, Authorization: `Bearer ${accessToken}` };
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
