// The updater as the UI sees it (ADR-0019): the status from the backend,
// kept current by `runtime://update`, and the user's actions.

import { useCallback, useEffect, useState } from "react";
import { updateEvents } from "./events";
import { errorMessage, updateApi } from "./runtime";
import type { UpdateInfo, UpdateStatus } from "./types";

export interface Updates {
  status: UpdateStatus | null;
  /** Bytes so far and the size, while downloading. */
  progress: { downloaded: number; total: number | null } | null;
  busy: "check" | "install" | null;
  error: string | null;
  /** Result of the last check the user asked for: null = up to date. */
  checked: UpdateInfo | null | undefined;
  check: () => Promise<void>;
  install: () => Promise<void>;
  restart: () => Promise<void>;
  setAutoCheck: (on: boolean) => Promise<void>;
}

export function useUpdates(enabled: boolean): Updates {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [progress, setProgress] = useState<Updates["progress"]>(null);
  const [busy, setBusy] = useState<Updates["busy"]>(null);
  const [error, setError] = useState<string | null>(null);
  const [checked, setChecked] = useState<UpdateInfo | null | undefined>(undefined);

  const refresh = useCallback(async () => {
    try {
      setStatus(await updateApi.status());
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  useEffect(() => {
    if (!enabled) return;
    void refresh();
    return updateEvents.subscribe((event) => {
      if (event.kind === "progress") {
        setProgress({ downloaded: event.downloaded, total: event.total });
        return;
      }
      if (event.kind === "failed") setError(event.message);
      if (event.kind === "installed") setProgress(null);
      void refresh();
    });
  }, [enabled, refresh]);

  const check = useCallback(async () => {
    setBusy("check");
    setError(null);
    try {
      setChecked(await updateApi.check());
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
      await refresh();
    }
  }, [refresh]);

  const install = useCallback(async () => {
    setBusy("install");
    setError(null);
    setProgress({ downloaded: 0, total: null });
    try {
      await updateApi.install();
    } catch (err) {
      setError(errorMessage(err));
      setProgress(null);
    } finally {
      setBusy(null);
      await refresh();
    }
  }, [refresh]);

  const restart = useCallback(async () => {
    try {
      await updateApi.restart();
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  const setAutoCheck = useCallback(async (on: boolean) => {
    try {
      setStatus(await updateApi.saveSettings(on));
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  return { status, progress, busy, error, checked, check, install, restart, setAutoCheck };
}
