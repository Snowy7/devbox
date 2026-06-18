import { contextBridge, ipcRenderer } from "electron";
import type { AlphaState } from "./shared/alphaState";

contextBridge.exposeInMainWorld("devbox", {
  getAlphaState: (): Promise<AlphaState> => ipcRenderer.invoke("devbox:alpha-state")
});
