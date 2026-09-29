// Registered AI providers, their availability and the provider sessions.
// Refreshed when the backend reports a change (no polling).

import { useCallback, useEffect, useState } from "react";
import { auditEvents, streamEvents } from "./events";
import { applyEvent, emptyTranscript, fromSnapshot, type Transcript } from "./transcript";
import { errorMessage, providerApi, sessionApi } from "./runtime";
import type { ProvidersView, ProviderStatus, SessionEvent, SessionInfo } from "./types";

export type ProviderHealth = { status: ProviderStatus } | { error: string } | { checking: true };

export interface Providers {
  view: ProvidersView | null;
  health: Record<string, ProviderHealth>;
  sessions: SessionInfo[];
  error: string | null;
  refresh: () => Promise<void>;
  select: (id: string) => Promise<void>;
  inspect: (id: string) => Promise<void>;
}

export function useProviders(enabled: boolean): Providers {
  const [view, setView] = useState<ProvidersView | null>(null);
  const [health, setHealth] = useState<Record<string, ProviderHealth>>({});
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refreshSessions = useCallback(async () => {
    setSessions(await sessionApi.list());
  }, []);

  const refresh = useCallback(async () => {
    const [nextView, nextSessions] = await Promise.all([providerApi.list(), sessionApi.list()]);
    setView(nextView);
    setSessions(nextSessions);
  }, []);

  const inspect = useCallback(async (id: string) => {
    setHealth((all) => ({ ...all, [id]: { checking: true } }));
    try {
      const status = await providerApi.inspect(id);
      setHealth((all) => ({ ...all, [id]: { status } }));
    } catch (e) {
      setHealth((all) => ({ ...all, [id]: { error: errorMessage(e) } }));
    }
  }, []);

  const select = useCallback(async (id: string) => {
    try {
      setView(await providerApi.select(id));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (!enabled) return;
    refresh()
      .then(() => setError(null))
      .catch((e) => setError(errorMessage(e)));
    const reloadSessions = () => void refreshSessions().catch((e) => setError(errorMessage(e)));
    const unsubscribeStream = streamEvents.subscribe((event) => {
      if (event.type !== "session") return;
      // Turn start/end, close and resume all change the status.
      if (event.event.type === "statusChanged" || event.event.type === "subagentSpawned") reloadSessions();
    });
    const unsubscribeAudit = auditEvents.subscribe((event) => {
      if (event.kind === "SESSION_STARTED") reloadSessions();
      else if (event.kind === "PROVIDER_SWITCHED") void refresh().catch((e) => setError(errorMessage(e)));
    });
    return () => {
      unsubscribeStream();
      unsubscribeAudit();
    };
  }, [enabled, refresh, refreshSessions]);

  // Check availability once per provider.
  const ids = view?.providers.map((p) => p.id).join("|") ?? "";
  useEffect(() => {
    if (!ids) return;
    for (const id of ids.split("|")) void inspect(id);
  }, [ids, inspect]);

  return { view, health, sessions, error, refresh, select, inspect };
}

/** Live transcript of one session: snapshot + stream events merged by seq. */
export function useSessionTranscript(sessionId: string, enabled: boolean): { transcript: Transcript; error: string | null } {
  const [transcript, setTranscript] = useState<Transcript>(emptyTranscript);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let loaded = false;
    let buffered: Array<{ seq: number; at: string; event: SessionEvent }> = [];
    setTranscript(emptyTranscript);
    const unsubscribe = streamEvents.subscribe((event) => {
      if (event.type !== "session" || event.sessionId !== sessionId) return;
      const entry = { seq: event.seq, at: new Date().toISOString(), event: event.event };
      if (!loaded) {
        buffered.push(entry);
        return;
      }
      setTranscript((t) => applyEvent(t, entry.seq, entry.at, entry.event));
    });
    streamEvents
      .ready()
      .then(() => sessionApi.get(sessionId))
      .then((snapshot) => {
        if (disposed) return;
        let next = fromSnapshot(snapshot);
        for (const entry of buffered) next = applyEvent(next, entry.seq, entry.at, entry.event);
        buffered = [];
        loaded = true;
        setTranscript(next);
        setError(snapshot.truncated ? "Início da conversa descartado (limite do transcript)." : null);
      })
      .catch((e) => {
        if (!disposed) setError(errorMessage(e));
      });
    return () => {
      disposed = true;
      unsubscribe();
    };
  }, [sessionId, enabled]);

  return { transcript, error };
}
