// The user's API connections, refreshed when history reports a change.

import { useCallback, useEffect, useState } from "react";
import { auditEvents } from "./events";
import { connectionApi, errorMessage } from "./runtime";
import type { ConnectionsView } from "./types";

export interface Connections {
  view: ConnectionsView | null;
  error: string | null;
  refresh: () => Promise<void>;
}

export function useConnections(enabled: boolean): Connections {
  const [view, setView] = useState<ConnectionsView | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setView(await connectionApi.list());
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (!enabled) return;
    void refresh();
    return auditEvents.subscribe((event) => {
      if (event.kind === "CONNECTION_SAVED" || event.kind === "CONNECTION_REMOVED") void refresh();
    });
  }, [enabled, refresh]);

  return { view, error, refresh };
}
