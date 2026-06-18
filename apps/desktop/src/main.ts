import { app, BrowserWindow, Menu, Tray, ipcMain, nativeImage } from "electron";
import path from "node:path";
import { alphaStateFixture } from "./shared/alphaState";

let mainWindow: BrowserWindow | null = null;
let tray: Tray | null = null;

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1120,
    height: 760,
    minWidth: 900,
    minHeight: 620,
    title: "Devbox Alpha",
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
  tray = new Tray(nativeImage.createEmpty());
  tray.setToolTip("Devbox alpha: warning");
  tray.setContextMenu(
    Menu.buildFromTemplate([
      {
        label: "Show Devbox",
        click: () => {
          mainWindow?.show();
        }
      },
      { label: "Status: warning", enabled: false },
      { label: "Sync mode: no-network", enabled: false },
      { type: "separator" },
      {
        label: "Quit",
        click: () => app.quit()
      }
    ])
  );
}

ipcMain.handle("devbox:alpha-state", () => alphaStateFixture);

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
