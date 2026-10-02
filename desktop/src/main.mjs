import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { app, BrowserWindow, dialog, Menu, net, session, shell } from "electron";

import {
  APP_URL,
  authorizeRequestHeaders,
  classifyNavigation,
  createPermissionPolicy,
  createWindowOptions,
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

const development = !app.isPackaged;
const icon = app.isPackaged
  ? join(process.resourcesPath, "icon.png")
  : fileURLToPath(new URL("../assets/icon.png", import.meta.url));

let mainWindow = null;
let accessToken = null;
let refreshToken = null;
let pendingAuthorization = null;
let refreshTimer = null;

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

function installAuthorizationHeader() {
  session.defaultSession.webRequest.onBeforeSendHeaders(
    { urls: ["https://getbuild.ing/*"] },
    (details, callback) => {
      callback({
        requestHeaders: authorizeRequestHeaders(
          details.url,
          details.requestHeaders,
          accessToken,
          details.resourceType,
        ),
      });
    },
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
    throw new Error(payload.error_description || "Build sign-in failed");
  }
  if (!payload.access_token || !payload.refresh_token) {
    throw new Error("Build sign-in returned incomplete credentials");
  }
  return payload;
}

function useTokens(payload) {
  accessToken = payload.access_token;
  refreshToken = payload.refresh_token;
  if (refreshTimer) clearTimeout(refreshTimer);
  const refreshAfter = Math.max(30, Number(payload.expires_in || 900) - 60);
  refreshTimer = setTimeout(() => void refreshTokens(), refreshAfter * 1000);
}

async function refreshTokens() {
  if (!refreshToken) return;
  try {
    useTokens(
      await requestTokens(
        new URLSearchParams({
          grant_type: "refresh_token",
          refresh_token: refreshToken,
          client_id: CLIENT_ID,
        }),
      ),
    );
  } catch (error) {
    accessToken = null;
    refreshToken = null;
    await showAuthenticationError(error);
  }
}

async function showAuthenticationError(error) {
  const result = await dialog.showMessageBox({
    type: "error",
    title: "Build sign-in failed",
    message: "Build could not complete sign-in.",
    detail: error instanceof Error ? error.message : String(error),
    buttons: ["Try Again", "Quit"],
    defaultId: 0,
    cancelId: 1,
  });
  if (result.response === 0) await beginAuthentication();
  else app.quit();
}

async function beginAuthentication() {
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
  pendingAuthorization = null;
  try {
    const { code } = parseAuthorizationCallback(url, authorization.state);
    useTokens(
      await requestTokens(
        tokenRequestBody({ code, verifier: authorization.verifier }),
      ),
    );
    if (!mainWindow) mainWindow = createWindow();
    else loadInsideElectron(mainWindow, APP_URL);
  } catch (error) {
    await showAuthenticationError(error);
  }
}

function installApplicationMenu() {
  const template = [
    ...(process.platform === "darwin"
      ? [{ role: "appMenu" }]
      : []),
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
    installPermissionPolicy();
    installAuthorizationHeader();
    installApplicationMenu();
    void beginAuthentication();

    app.on("activate", () => {
      if (mainWindow) return;
      if (accessToken) mainWindow = createWindow();
      else if (!pendingAuthorization) void beginAuthentication();
    });
  });

  app.on("window-all-closed", () => {
    if (process.platform !== "darwin") app.quit();
  });
}
