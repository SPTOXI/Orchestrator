// Shared view of open terminals and managed processes, refreshed only when
// the runtime reports a change (no polling).

import { useCallback, useEffect, useState } from "react";
import { auditEvents } from "./events";
import { processApi, terminalApi } from "./runtime";
import type { ProcessInfo, TerminalInfo } from "./types";

const LIFECYCLE_TOOLS = /^(terminal\.(create|close)|process\.(start|stop))$/;

export interface RuntimeSessions {
  terminals: TerminalInfo[];
  processes: ProcessInfo[];
  /** True after the first successful refresh. */
  loaded: boolean;
  refresh: () => Promise<void>;
}

export function useRuntimeSessions(enabled: boolean): RuntimeSessions {
  const [terminals, setTerminals] = useState<TerminalInfo[]>([]);
  const [processes, setProcesses] = useState<ProcessInfo[]>([]);
  const [loaded, setLoaded] = useState(false);

  const refresh = useCallback(async () => {
    const [nextTerminals, nextProcesses] = await Promise.all([
      terminalApi.list(),
      processApi.list(),
    ]);
    setTerminals(nextTerminals);
    setProcesses(nextProcesses);
    setLoaded(true);
  }, []);

  useEffect(() => {
    if (!enabled) return;
    const reload = () => void refresh().catch(() => undefined);
    reload();
    return auditEvents.subscribe((event) => {
      if (event.kind === "PROCESS_EXITED" || event.kind === "TERMINAL_EXITED") {
        reload();
      } else if (event.kind === "TOOL_CALLED" && LIFECYCLE_TOOLS.test(String(event.data.tool))) {
        reload();
      } else if (event.kind === "COMMAND_EXECUTED" && event.data.background === true) {
        // Processes started by other tools (e.g. package.run with background).
        reload();
      }
    });
  }, [enabled, refresh]);

  return { terminals, processes, loaded, refresh };
}
