import { contextBridge, ipcRenderer } from "electron";
import type { AlphaState } from "./shared/alphaState";

contextBridge.exposeInMainWorld("bindhub", {
  getAlphaState: (): Promise<AlphaState> => ipcRenderer.invoke("bindhub:alpha-state")
});
