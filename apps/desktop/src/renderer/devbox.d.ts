import type { AlphaState } from "../shared/alphaState";

declare global {
  interface Window {
    devbox?: {
      getAlphaState: () => Promise<AlphaState>;
    };
  }
}

export {};
