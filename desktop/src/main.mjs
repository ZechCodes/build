import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { app, BaseWindow, BrowserWindow, dialog, Menu, safeStorage, session, shell } from "electron";

import {
  APP_URL,
  classifyNavigation,
  createPermissionPolicy,
  createWindowOptions,
  installAuthorizationHeader,
  installNavigationPolicy,
  installUnauthorizedResponsePolicy,
  isAuthenticationDocument,
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
let exchangingCode = false;
let refreshTimer = null;
let tokenStore = null;
let refreshInFlight = null;
let starting = true;
let signingOut = false;
let credentialGeneration = 0;
let tokenSession = null;
const tokenRequests = new Set();
const SIGN_OUT_WAIT_MS = 5_000;
let signOutComplete = false;
let unauthorizedRecovery = null;
const windowNavigations = new WeakMap();

function openOutsideElectron(url) {
  if (classifyNavigation(url) !== "external") return;
  void shell.openExternal(url).catch((error) => {
    console.error("could not open an external link", error);
  });
}

function loadInsideElectron(window, url) {
  const recovery = unauthorizedRecovery;
  const generation = credentialGeneration;
  const navigation = windowNavigations.get(window);
  const attempt = ++navigation.loadAttempt;
  const load = { canceled: false };
  navigation.currentLoad = load;
  void window.loadURL(url).catch((error) => {
    if (isCanceledDocumentLoad(window, url, error.errno, error.code, load)) return;
    if (attempt !== navigation.loadAttempt) return;
    if (generation === credentialGeneration && failRecoveryLoad(window, recovery)) return;
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
  const controller = new AbortController();
  tokenRequests.add(controller);
  try {
    // Token responses must never share the renderer's persistent session.
    const response = await tokenSession.fetch(TOKEN_URL, {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: body.toString(),
      credentials: "omit",
      cache: "no-store",
      signal: controller.signal,
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
  } finally {
    tokenRequests.delete(controller);
  }
}

function persistRefreshToken(token) {
  let persisted = false;
  try {
    persisted = tokenStore.save(token);
  } catch {
    console.error("could not save encrypted Build sign-in; this launch will use memory only");
  }
  if (persisted) return;
  // A new in-memory account must never leave an older account restorable.
  try {
    tokenStore.clear();
  } catch {
    throw new Error("Build could not remove the previous saved sign-in. Please try again.");
  }
}

function useTokens(payload) {
  persistRefreshToken(payload.refresh_token);
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
  refreshInFlight = null;
  if (refreshTimer) clearTimeout(refreshTimer);
  refreshTimer = null;
}

function openAuthenticatedWindow() {
  if (signingOut) return;
  if (unauthorizedRecovery) {
    const phase = unauthorizedRecovery.phase;
    if (phase === "refresh" || phase === "oauth") {
      unauthorizedRecovery.phase = `retry-${phase}`;
      unauthorizedRecovery.rejectedNavigation = windowNavigations.get(mainWindow)?.sequence ?? 0;
    }
  }
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

async function refreshTokens({ unauthorized = false } = {}) {
  if (!refreshToken) return false;
  const generation = credentialGeneration;
  const ownsFailurePolicy = !refreshInFlight;
  const request = refreshInFlight || performRefresh(generation);
  refreshInFlight = request;
  let error;
  try {
    return await request;
  } catch (failure) {
    error = failure;
  } finally {
    if (refreshInFlight === request) refreshInFlight = null;
  }
  if (generation !== credentialGeneration) return false;
  // An observed document rejection owns the pending exchange's failure policy,
  // including when the scheduled refresh started that exchange first.
  if (unauthorized || unauthorizedRecovery || !ownsFailurePolicy) return false;
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

function recoveryIsCurrent(recovery) {
  return !signingOut && unauthorizedRecovery === recovery && mainWindow === recovery.window;
}

async function authenticateAfterRejection(recovery) {
  recovery.phase = "oauth";
  recovery.rejectedNavigation = windowNavigations.get(recovery.window).sequence;
  await beginAuthentication();
}

async function failRejectedAuthentication(recovery, message = "Build rejected the sign-in. Please try again.") {
  recovery.phase = "failed";
  clearCredentials();
  await showAuthenticationError(new Error(message));
}

async function retryRejectedRecovery(recovery, navigation) {
  // Canceled headers can also produce a navigation notification. A retry can
  // consume the next recovery step only after a new main-frame navigation.
  if (!recovery.phase.startsWith("retry-") || navigation.sequence <= recovery.rejectedNavigation) return;
  recovery.window.hide();
  if (recovery.phase === "retry-refresh") await authenticateAfterRejection(recovery);
  else await failRejectedAuthentication(recovery);
}

async function recoverUnauthorized(window, requestId, canceledUrl) {
  if (!window || signingOut || window !== mainWindow) return;
  const navigation = windowNavigations.get(window);
  if (requestId !== undefined) {
    if (navigation.rejectedRequests.has(requestId)) return;
    navigation.rejectedRequests.add(requestId);
  }
  if (canceledUrl) {
    navigation.canceledUrls.add(canceledUrl);
    if (navigation.currentLoad) navigation.currentLoad.canceled = true;
  }
  const previous = unauthorizedRecovery;
  if (previous) return retryRejectedRecovery(previous, navigation);
  const recovery = { window, phase: "refresh", rejectedNavigation: navigation.sequence };
  unauthorizedRecovery = recovery;
  window.hide();
  const generation = credentialGeneration;
  const refreshed = await refreshTokens({ unauthorized: true });
  if (!recoveryIsCurrent(recovery) || generation !== credentialGeneration) return;
  if (refreshed) openAuthenticatedWindow();
  else await authenticateAfterRejection(recovery);
}

function isCanceledDocumentLoad(window, url, errorCode, errorName, load) {
  if (errorCode === -3 || errorName === "ERR_ABORTED") return true;
  // Electron's canceled onHeadersReceived callback normally reports -20;
  // recognize only URLs whose 401 response this window actually canceled.
  const blocked = errorCode === -20 || errorName === "ERR_BLOCKED_BY_CLIENT";
  const canceled = load ? load.canceled : windowNavigations.get(window)?.canceledUrls.has(url);
  return blocked && canceled;
}

function failRecoveryLoad(window, recovery = unauthorizedRecovery) {
  if (!recovery || !recoveryIsCurrent(recovery) || window !== recovery.window) return false;
  if (!recovery.phase.startsWith("retry-")) return false;
  void failRejectedAuthentication(recovery, "Build could not load the signed-in desktop. Please try again.");
  return true;
}

function didNavigateMainFrame(window, navigation, url, statusCode) {
  if (window !== mainWindow || signingOut) return;
  if (statusCode === 401 && isAuthenticationDocument(url)) {
    void recoverUnauthorized(window);
    return;
  }
  const recovery = unauthorizedRecovery;
  if (!recovery || !recovery.phase.startsWith("retry-")) return;
  if (navigation.sequence <= recovery.rejectedNavigation || classifyNavigation(url) !== "internal") return;
  if (statusCode >= 400) {
    failRecoveryLoad(window, recovery);
    return;
  }
  if (!(statusCode >= 200 && statusCode < 300)) return;
  unauthorizedRecovery = null;
  navigation.rejectedRequests.clear();
  navigation.canceledUrls.clear();
  window.show();
}

function observeMainFrameNavigation(window) {
  const navigation = { sequence: 0, loadAttempt: 0, rejectedRequests: new Set(), canceledUrls: new Set() };
  windowNavigations.set(window, navigation);
  window.webContents.on("did-start-navigation", (_event, _url, _inPlace, isMainFrame) => {
    if (isMainFrame) navigation.sequence++;
  });
  window.webContents.on("did-navigate", (_event, url, statusCode) => {
    didNavigateMainFrame(window, navigation, url, statusCode);
  });
  window.webContents.on("did-fail-load", (_event, errorCode, _description, url, isMainFrame) => {
    if (!isMainFrame || isCanceledDocumentLoad(window, url, errorCode)) return;
    if (classifyNavigation(url) === "internal") failRecoveryLoad(window);
  });
}

async function startAuthentication() {
  if (signingOut || unauthorizedRecovery) return;
  try {
    refreshToken ||= tokenStore.load();
    if (!refreshToken) await beginAuthentication();
    else {
      const refreshed = await refreshTokens();
      if (refreshed && !unauthorizedRecovery) openAuthenticatedWindow();
    }
  } catch {
    await showAuthenticationError(new Error("Build could not read the saved sign-in."), startAuthentication);
  }
}

async function waitForSignOutOperation(operation) {
  let timer;
  try {
    const timeout = new Promise((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error("Build sign-out timed out.")), SIGN_OUT_WAIT_MS);
    });
    return await Promise.race([operation(), timeout]);
  } finally {
    clearTimeout(timer);
  }
}

async function reportSignOutFailures(failures) {
  const title = "Build sign-out failed";
  const message = `Build could not clear ${failures.join(" and ")}. Please try signing out again after restarting.`;
  // Unparented macOS message boxes block synchronously. A native parent
  // without a renderer lets the deadline and AbortSignal close this dialog.
  const parent = new BaseWindow({ title: message, width: 480, height: 180 });
  const controller = new AbortController();
  try {
    await waitForSignOutOperation(() => dialog.showMessageBox(parent, {
      type: "error", title, message, buttons: ["Quit"], signal: controller.signal,
    }));
  } catch {
    console.error(message);
  } finally {
    controller.abort();
    if (!parent.isDestroyed()) parent.destroy();
  }
}

async function signOut() {
  if (signingOut) return;
  signingOut = true;
  unauthorizedRecovery = null;
  clearCredentials();
  for (const controller of tokenRequests) controller.abort();
  // Retire the renderer before clearing persistent storage so it cannot
  // rewrite account content while cleanup awaits Electron.
  mainWindow?.destroy();
  const failures = [];
  try {
    tokenStore.clear();
  } catch {
    failures.push("the saved sign-in");
  }
  const cleanupOperations = [
    ["browser storage", () => session.defaultSession.clearStorageData()],
    ["the HTTP cache", () => session.defaultSession.clearCache()],
  ];
  for (const [name, cleanup] of cleanupOperations) {
    try {
      await waitForSignOutOperation(cleanup);
    } catch {
      failures.push(name);
    }
  }
  try {
    if (failures.length) await reportSignOutFailures(failures);
  } finally {
    signOutComplete = true;
    app.quit();
  }
}

async function showAuthenticationError(error, retry = beginAuthentication) {
  if (signingOut) return;
  const result = await dialog.showMessageBox({
    type: "error",
    title: "Build sign-in failed",
    message: "Build could not complete sign-in.",
    detail: error instanceof Error ? error.message : String(error),
    buttons: ["Try Again", "Quit"],
    defaultId: 0,
    cancelId: 1,
  });
  if (signingOut) return;
  if (result.response === 0) await retry();
  else app.quit();
}

async function beginAuthentication() {
  if (signingOut || pendingAuthorization || exchangingCode) return;
  if (unauthorizedRecovery) unauthorizedRecovery.phase = "oauth";
  // A 401 recovery retires the rejected credentials and any late refresh result.
  clearCredentials();
  pendingAuthorization = createAuthorizationRequest();
  try {
    await shell.openExternal(pendingAuthorization.url);
  } catch (error) {
    pendingAuthorization = null;
    await showAuthenticationError(error);
  }
}

async function handleAuthorizationCallback(url) {
  if (!pendingAuthorization) return;
  const authorization = pendingAuthorization;
  const generation = credentialGeneration;
  pendingAuthorization = null;
  exchangingCode = true;
  try {
    const { code } = parseAuthorizationCallback(url, authorization.state);
    const payload = await requestTokens(
      tokenRequestBody({ code, verifier: authorization.verifier }),
    );
    if (generation !== credentialGeneration) return;
    useTokens(payload);
    openAuthenticatedWindow();
  } catch (error) {
    exchangingCode = false;
    if (generation === credentialGeneration) await showAuthenticationError(error);
  } finally {
    exchangingCode = false;
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

  observeMainFrameNavigation(window);
  installNavigationPolicy(window.webContents, {
    openExternal: openOutsideElectron,
    loadInternal: (url) => loadInsideElectron(window, url),
    signOut,
    signIn: beginAuthentication,
  });
  window.once("ready-to-show", () => {
    if (window === mainWindow && !signingOut && !unauthorizedRecovery) window.show();
  });
  window.on("closed", () => {
    if (mainWindow === window) mainWindow = null;
    if (unauthorizedRecovery?.window === window) unauthorizedRecovery = null;
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
    if (!mainWindow || unauthorizedRecovery) return;
    if (mainWindow.isMinimized()) mainWindow.restore();
    mainWindow.show();
    mainWindow.focus();
  });

  app.whenReady().then(() => {
    tokenSession = session.fromPartition("build-oauth", { cache: false });
    tokenStore = createTokenStore({ userData: app.getPath("userData"), safeStorage });
    installPermissionPolicy();
    installAuthorizationHeader(session.defaultSession.webRequest, () => accessToken);
    installUnauthorizedResponsePolicy(
      session.defaultSession.webRequest,
      () => mainWindow?.webContents,
      (requestId, url) => void recoverUnauthorized(mainWindow, requestId, url),
    );
    installApplicationMenu();
    void startAuthentication().finally(() => { starting = false; });

    app.on("activate", () => {
      if (mainWindow || starting || exchangingCode || unauthorizedRecovery) return;
      if (accessToken) mainWindow = createWindow();
      else if (!pendingAuthorization) void startAuthentication();
    });
  });

  app.on("before-quit", (event) => {
    if (signingOut && !signOutComplete) event.preventDefault();
  });

  app.on("window-all-closed", () => {
    if (process.platform !== "darwin" && !pendingAuthorization) app.quit();
  });
}
