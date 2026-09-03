import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { app, BrowserWindow, Menu, session, shell } from "electron";

import {
  APP_URL,
  classifyNavigation,
  createPermissionPolicy,
  createWindowOptions,
} from "./security-policy.mjs";

const development = !app.isPackaged;
const icon = app.isPackaged
  ? join(process.resourcesPath, "icon.png")
  : fileURLToPath(new URL("../../spa/public/icon-512.png", import.meta.url));

let mainWindow = null;

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

function protectWebContents(window) {
  const handleNavigation = (event, url) => {
    const destination = classifyNavigation(url);
    if (destination === "internal") return;

    event.preventDefault();
    if (destination === "external") openOutsideElectron(url);
  };

  window.webContents.on("will-navigate", handleNavigation);
  window.webContents.on("will-redirect", handleNavigation);

  window.webContents.on("will-attach-webview", (event) => event.preventDefault());
  window.webContents.setWindowOpenHandler(({ url }) => {
    const destination = classifyNavigation(url);
    if (destination === "internal") loadInsideElectron(window, url);
    if (destination === "external") openOutsideElectron(url);
    return { action: "deny" };
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

  protectWebContents(window);
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

  app.on("second-instance", () => {
    if (!mainWindow) return;
    if (mainWindow.isMinimized()) mainWindow.restore();
    mainWindow.show();
    mainWindow.focus();
  });

  app.whenReady().then(() => {
    installPermissionPolicy();
    installApplicationMenu();
    mainWindow = createWindow();

    app.on("activate", () => {
      if (!mainWindow) mainWindow = createWindow();
    });
  });

  app.on("window-all-closed", () => {
    if (process.platform !== "darwin") app.quit();
  });
}
