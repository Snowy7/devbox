import { app, BrowserWindow, Menu, Tray, ipcMain, nativeImage } from "electron";
import path from "node:path";
import { buildAlphaStateFromEnv } from "./shared/alphaState";

let mainWindow: BrowserWindow | null = null;
let tray: Tray | null = null;

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1240,
    height: 820,
    minWidth: 960,
    minHeight: 680,
    title: "Bindhub Alpha",
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      preload: path.join(__dirname, "preload.js")
    }
  });

  const devServerUrl = process.env.VITE_DEV_SERVER_URL;
  if (devServerUrl) {
    void mainWindow.loadURL(devServerUrl);
  } else {
    void mainWindow.loadFile(path.join(__dirname, "../renderer/index.html"));
  }
}

function createTray() {
  const state = buildAlphaStateFromEnv(process.env);
  tray = new Tray(nativeImage.createEmpty());
  tray.setToolTip(`Bindhub alpha: ${state.status}`);
  tray.setContextMenu(
    Menu.buildFromTemplate([
      {
        label: "Show Bindhub",
        click: () => {
          mainWindow?.show();
        }
      },
      { label: `Status: ${state.status}`, enabled: false },
      { label: `Remote: ${state.remote.kind}`, enabled: false },
      { label: `Live sync: ${state.liveSync.status}`, enabled: false },
      { type: "separator" },
      {
        label: "Quit",
        click: () => app.quit()
      }
    ])
  );
}

ipcMain.handle("bindhub:alpha-state", () => buildAlphaStateFromEnv(process.env));

void app.whenReady().then(() => {
  createWindow();
  createTray();

  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      createWindow();
    }
  });
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") {
    app.quit();
  }
});
