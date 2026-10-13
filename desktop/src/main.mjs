import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { app, BrowserWindow, dialog, Menu, net, safeStorage, session, shell } from "electron";

import {
  APP_URL,
  classifyNavigation,
  createPermissionPolicy,
  createWindowOptions,
  installAuthorizationHeader,
  installNavigationPolicy,
} from "./security-policy.mjs";
import {
  CLIENT_ID,
  createAuthorizationRequest,
  parseAuthorizationCallback,
  REDIRECT_URI,
  tokenRequestBody,
  TOKEN_URL,
} from "./oauth.mjs";

import { createTokenStore } from "./token-store.mjs";

const development = !app.isPackaged;
const icon = app.isPackaged
  ? join(process.resourcesPath, "icon.png")
  : fileURLToPath(new URL("../assets/icon.png", import.meta.url));

let mainWindow = null;
let accessToken = null;
let refreshToken = null;
let pendingAuthorization = null;
let refreshTimer = null;
let tokenStore = null;
let refreshInFlight = null;
let starting = true;
let signingOut = false;
let credentialGeneration = 0;

function openOutsideElectron(url) {
  if (classifyNavigation(url) !== "external") return;
  void shell.openExternal(url).catch((error) => {
    console.error("could not open an external link", error);
  });
}

function loadInsideElectron(window, url) {
  void window.loadURL(url).catch((error) => {
    console.error("could not load the Build web client", error);
  });
}

function installPermissionPolicy() {
  const permissionAllowed = createPermissionPolicy();

  session.defaultSession.setPermissionCheckHandler(
    (_webContents, permission, requestingOrigin) =>
      permissionAllowed(permission, requestingOrigin),
  );
  session.defaultSession.setPermissionRequestHandler(
    (_webContents, permission, callback, details) =>
      callback(permissionAllowed(permission, details.requestingUrl)),
  );
}

function authorizationCallbackFromArguments(arguments_) {
  return arguments_.find((argument) => argument.startsWith(`${REDIRECT_URI}?`));
}

async function requestTokens(body) {
  const response = await net.fetch(TOKEN_URL, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: body.toString(),
  });
  const payload = await response.json();
  if (!response.ok) {
    const error = new Error("Build could not obtain credentials. Please try again.");
    error.code = payload.error;
    throw error;
  }
  if (!payload.access_token || !payload.refresh_token) {
    throw new Error("Build sign-in returned incomplete credentials");
  }
  return payload;
}

function useTokens(payload) {
  try {
    tokenStore.save(payload.refresh_token);
  } catch {
    // Rotation has already invalidated the previous token on the server.
    // Keep this launch usable in memory, but don't leave stale credentials.
    try { tokenStore.clear(); } catch { /* Disk may also be unavailable. */ }
    console.error("could not save encrypted Build sign-in; this launch will use memory only");
  }
  accessToken = payload.access_token;
  refreshToken = payload.refresh_token;
  if (refreshTimer) clearTimeout(refreshTimer);
  const refreshAfter = Math.max(30, Number(payload.expires_in || 900) - 60);
  refreshTimer = setTimeout(() => void refreshTokens(), refreshAfter * 1000);
}

function clearCredentials() {
  credentialGeneration++;
  accessToken = null;
  refreshToken = null;
  pendingAuthorization = null;
  if (refreshTimer) clearTimeout(refreshTimer);
  refreshTimer = null;
}

function openAuthenticatedWindow() {
  if (!mainWindow) mainWindow = createWindow();
  else loadInsideElectron(mainWindow, APP_URL);
}

async function performRefresh(generation) {
  const payload = await requestTokens(new URLSearchParams({
    grant_type: "refresh_token",
    refresh_token: refreshToken,
    client_id: CLIENT_ID,
  }));
  if (generation !== credentialGeneration) return false;
  useTokens(payload);
  return true;
}

async function refreshTokens() {
  if (!refreshToken || signingOut) return false;
  if (refreshInFlight) return refreshInFlight;
  const generation = credentialGeneration;
  refreshInFlight = performRefresh(generation);
  let error;
  try {
    return await refreshInFlight;
  } catch (failure) {
    error = failure;
  } finally {
    refreshInFlight = null;
  }
  if (generation !== credentialGeneration) return false;
  if (error.code === "invalid_grant") {
    clearCredentials();
    try {
      tokenStore.clear();
    } catch {
      await showAuthenticationError(new Error("Build could not remove the saved sign-in."), startAuthentication);
      return false;
    }
    // Set pendingAuthorization before destroying the old account's window.
    const authentication = beginAuthentication();
    mainWindow?.destroy();
    await authentication;
  } else {
    await showAuthenticationError(error, startAuthentication);
  }
  return false;
}

async function startAuthentication() {
  if (signingOut) return;
  try {
    refreshToken ||= tokenStore.load();
    if (!refreshToken) await beginAuthentication();
    else if (await refreshTokens()) openAuthenticatedWindow();
  } catch {
    await showAuthenticationError(new Error("Build could not read the saved sign-in."), startAuthentication);
  }
}

async function signOut() {
  signingOut = true;
  clearCredentials();
  try {
    tokenStore.clear();
  } catch {
    await dialog.showMessageBox({
      type: "error",
      title: "Build sign-out failed",
      message: "Build could not remove the saved sign-in. Please try signing out again after restarting.",
      buttons: ["Quit"],
    });
  }
  mainWindow?.destroy();
  app.quit();
}

async function showAuthenticationError(error, retry = beginAuthentication) {
  const result = await dialog.showMessageBox({
    type: "error",
    title: "Build sign-in failed",
    message: "Build could not complete sign-in.",
    detail: error instanceof Error ? error.message : String(error),
    buttons: ["Try Again", "Quit"],
    defaultId: 0,
    cancelId: 1,
  });
  if (result.response === 0 && !signingOut) await retry();
  else app.quit();
}

async function beginAuthentication() {
  if (signingOut || pendingAuthorization) return;
  pendingAuthorization = createAuthorizationRequest();
  try {
    await shell.openExternal(pendingAuthorization.url);
  } catch (error) {
    await showAuthenticationError(error);
  }
}

async function handleAuthorizationCallback(url) {
  if (!pendingAuthorization) return;
  const authorization = pendingAuthorization;
  const generation = credentialGeneration;
  pendingAuthorization = null;
  try {
    const { code } = parseAuthorizationCallback(url, authorization.state);
    const payload = await requestTokens(
      tokenRequestBody({ code, verifier: authorization.verifier }),
    );
    if (generation !== credentialGeneration) return;
    useTokens(payload);
    openAuthenticatedWindow();
  } catch (error) {
    if (generation === credentialGeneration) await showAuthenticationError(error);
  }
}

function installApplicationMenu() {
  const template = [
    ...(process.platform === "darwin"
      ? [{ role: "appMenu" }]
      : []),
    { label: "Account", submenu: [{ label: "Sign Out", click: signOut }] },
    { role: "editMenu" },
    {
      label: "View",
      submenu: [
        { role: "reload" },
        { type: "separator" },
        { role: "resetZoom" },
        { role: "zoomIn" },
        { role: "zoomOut" },
        { type: "separator" },
        { role: "togglefullscreen" },
        ...(development
          ? [{ type: "separator" }, { role: "toggleDevTools" }]
          : []),
      ],
    },
    { role: "windowMenu" },
  ];

  Menu.setApplicationMenu(Menu.buildFromTemplate(template));
}

function createWindow() {
  const window = new BrowserWindow({
    ...createWindowOptions({ development }),
    icon,
    title: "Build",
  });

  installNavigationPolicy(window.webContents, {
    openExternal: openOutsideElectron,
    loadInternal: (url) => loadInsideElectron(window, url),
  });
  window.once("ready-to-show", () => window.show());
  window.on("closed", () => {
    if (mainWindow === window) mainWindow = null;
  });
  loadInsideElectron(window, APP_URL);
  return window;
}

const hasSingleInstanceLock = app.requestSingleInstanceLock();
if (!hasSingleInstanceLock) {
  app.quit();
} else {
  app.setAppUserModelId("ing.getbuild.desktop");
  app.setAsDefaultProtocolClient("getbuilding");

  app.on("open-url", (event, url) => {
    event.preventDefault();
    void handleAuthorizationCallback(url);
  });

  app.on("second-instance", (_event, commandLine) => {
    const callback = authorizationCallbackFromArguments(commandLine);
    if (callback) {
      void handleAuthorizationCallback(callback);
      return;
    }
    if (!mainWindow) return;
    if (mainWindow.isMinimized()) mainWindow.restore();
    mainWindow.show();
    mainWindow.focus();
  });

  app.whenReady().then(() => {
    tokenStore = createTokenStore({ userData: app.getPath("userData"), safeStorage });
    installPermissionPolicy();
    installAuthorizationHeader(session.defaultSession.webRequest, () => accessToken);
    installApplicationMenu();
    void startAuthentication().finally(() => { starting = false; });

    app.on("activate", () => {
      if (mainWindow || starting || signingOut) return;
      if (accessToken) mainWindow = createWindow();
      else if (!pendingAuthorization) void startAuthentication();
    });
  });

  app.on("window-all-closed", () => {
    if (process.platform !== "darwin" && !pendingAuthorization && !signingOut) app.quit();
  });
}
