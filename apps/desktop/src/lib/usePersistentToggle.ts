// A boolean the user sets in the UI (a collapsed section, an expanded
// card) that survives restarts. Stored per key in localStorage.

import { useState } from "react";

const PREFIX = "orchestrator.ui.";

function read(key: string, fallback: boolean): boolean {
  try {
    const raw = localStorage.getItem(PREFIX + key);
    return raw === null ? fallback : raw === "1";
  } catch {
    return fallback;
  }
}

export function usePersistentToggle(key: string, fallback: boolean): [boolean, (value: boolean) => void] {
  const [value, setValue] = useState(() => read(key, fallback));
  const set = (next: boolean) => {
    setValue(next);
    try {
      localStorage.setItem(PREFIX + key, next ? "1" : "0");
    } catch {
      // Storage unavailable: the choice lasts until the app closes.
    }
  };
  return [value, set];
}
