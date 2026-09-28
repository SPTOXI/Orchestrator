// Git status of the open project, refreshed when something may have changed
// it: git tools, file writes, commands, terminal activity (throttled) and
// window focus. No fixed-interval polling.

import { useCallback, useEffect, useRef, useState } from "react";
import { auditEvents, streamEvents } from "./events";
import { ToolCallError, gitApi } from "./runtime";
import type { GitStatusWithRemotes } from "./types";

/** Re-read status once terminal output has been quiet for this long… */
const TERMINAL_QUIET_MS = 1500;
/** …or at most this long after output started (continuous output). */
const TERMINAL_MAX_WAIT_MS = 15_000;
const EVENT_DELAY_MS = 250;

const REFRESH_EVENTS = new Set([
  "FILE_CHANGED",
  "COMMAND_EXECUTED",
  "PROCESS_EXITED",
  "GIT_COMMIT",
  "GIT_PUSH",
  "PROJECT_OPENED",
]);

export interface GitState {
  status: GitStatusWithRemotes | null;
  /** The folder is not inside a Git repository. */
  notRepo: boolean;
  error: string | null;
  refresh: () => void;
}

export function useGitStatus(path: string | null, enabled: boolean): GitState {
  const [status, setStatus] = useState<GitStatusWithRemotes | null>(null);
  const [notRepo, setNotRepo] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);
  /** Not a repository: skip automatic re-checks (each would be a failed call). */
  const notRepoRef = useRef(false);
  const again = useRef(false);
  const timer = useRef<number | undefined>(undefined);
  const terminalTimer = useRef<number | undefined>(undefined);
  const terminalSince = useRef<number | null>(null);

  const load = useCallback(async () => {
    if (!path) return;
    if (inFlight.current) {
      again.current = true;
      return;
    }
    inFlight.current = true;
    try {
      const next = await gitApi.status(path);
      setStatus(next);
      setNotRepo(false);
      notRepoRef.current = false;
      setError(null);
    } catch (e) {
      setStatus(null);
      if (e instanceof ToolCallError && e.kind === "NOT_FOUND") {
        setNotRepo(true);
        notRepoRef.current = true;
        setError(null);
      } else {
        setError(e instanceof Error ? e.message : String(e));
      }
    } finally {
      inFlight.current = false;
      if (again.current) {
        again.current = false;
        void load();
      }
    }
  }, [path]);

  const schedule = useCallback(
    (delay: number) => {
      if (timer.current !== undefined) return;
      timer.current = window.setTimeout(() => {
        timer.current = undefined;
        void load();
      }, delay);
    },
    [load],
  );

  useEffect(() => {
    setStatus(null);
    setNotRepo(false);
    setError(null);
    notRepoRef.current = false;
    if (!enabled || !path) return;
    void load();

    const offAudit = auditEvents.subscribe((event) => {
      if (notRepoRef.current && event.kind !== "PROJECT_OPENED") return;
      if (REFRESH_EVENTS.has(event.kind)) {
        schedule(EVENT_DELAY_MS);
      } else if (
        event.kind === "TOOL_CALLED" &&
        event.data.readOnly === false &&
        String(event.data.tool).startsWith("git.")
      ) {
        schedule(EVENT_DELAY_MS);
      }
    });
    // Trailing debounce with a maximum wait for terminal activity.
    const offStream = streamEvents.subscribe((event) => {
      if (event.type !== "terminalOutput" || notRepoRef.current) return;
      const now = Date.now();
      terminalSince.current ??= now;
      window.clearTimeout(terminalTimer.current);
      const delay = now - terminalSince.current >= TERMINAL_MAX_WAIT_MS ? 0 : TERMINAL_QUIET_MS;
      terminalTimer.current = window.setTimeout(() => {
        terminalSince.current = null;
        void load();
      }, delay);
    });
    const onFocus = () => schedule(0);
    window.addEventListener("focus", onFocus);
    return () => {
      offAudit();
      offStream();
      window.removeEventListener("focus", onFocus);
      window.clearTimeout(timer.current);
      window.clearTimeout(terminalTimer.current);
      timer.current = undefined;
      terminalSince.current = null;
    };
  }, [enabled, path, load, schedule]);

  return { status, notRepo, error, refresh: () => schedule(0) };
}
