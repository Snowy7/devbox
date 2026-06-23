import type { AlphaState } from "../shared/alphaState";

declare global {
  interface Window {
    bindhub?: {
      getAlphaState: () => Promise<AlphaState>;
    };
  }
}

export {};
