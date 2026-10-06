// Council settings and recent deliberations, refreshed when the history
// reports a change (ADR-0011).

import { useCallback, useEffect, useState } from "react";
import { auditEvents } from "./events";
import { councilApi, errorMessage } from "./runtime";
import type { CouncilSettings, CouncilView, Deliberation } from "./types";

export interface Council {
  view: CouncilView | null;
  /** Newest first. */
  history: Deliberation[];
  error: string | null;
  refresh: () => Promise<void>;
  refreshHistory: () => Promise<void>;
  /** Throws a ProviderCallError when the settings are invalid. */
  save: (settings: CouncilSettings) => Promise<void>;
}

export function useCouncil(enabled: boolean): Council {
  const [view, setView] = useState<CouncilView | null>(null);
  const [history, setHistory] = useState<Deliberation[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setView(await councilApi.get());
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  const refreshHistory = useCallback(async () => {
    try {
      setHistory(await councilApi.history());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  const save = useCallback(
    async (settings: CouncilSettings) => {
      await councilApi.save(settings);
      await refresh();
    },
    [refresh],
  );

  useEffect(() => {
    if (!enabled) return;
    void refresh();
    void refreshHistory();
    return auditEvents.subscribe((event) => {
      if (event.kind === "COUNCIL_CONFIGURED") void refresh();
      if (event.kind === "COUNCIL_DELIBERATED" || event.kind === "ROUTE_DECIDED") void refreshHistory();
    });
  }, [enabled, refresh, refreshHistory]);

  return { view, history, error, refresh, refreshHistory, save };
}
