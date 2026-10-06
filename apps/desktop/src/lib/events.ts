// Fan-out of runtime events: one Tauri listener per channel, any number of
// in-app subscribers.

import { listen } from "@tauri-apps/api/event";
import { isTauri } from "./runtime";
import type { AuditEvent, LocalEvent, StreamEvent, UpdateEvent } from "./types";

type Handler<T> = (event: T) => void;

export interface EventChannel<T> {
  /** Registers a handler; returns the unsubscribe function. */
  subscribe(handler: Handler<T>): () => void;
  /** Resolves once the underlying Tauri listener is registered. */
  ready(): Promise<void>;
  /** Delivers an event to every subscriber (also used by tests). */
  dispatch(event: T): void;
}

export function createChannel<T>(
  register: (deliver: Handler<T>) => Promise<unknown> | null,
): EventChannel<T> {
  const handlers = new Set<Handler<T>>();
  let registration: Promise<void> | null = null;

  const dispatch = (event: T) => {
    for (const handler of [...handlers]) handler(event);
  };

  const ready = () => {
    if (!registration) {
      const pending = register(dispatch);
      registration = pending ? pending.then(() => undefined) : Promise.resolve();
    }
    return registration;
  };

  return {
    subscribe(handler) {
      handlers.add(handler);
      void ready();
      return () => {
        handlers.delete(handler);
      };
    },
    ready,
    dispatch,
  };
}

function tauriChannel<T>(eventName: string): EventChannel<T> {
  return createChannel<T>((deliver) =>
    isTauri() ? listen<T>(eventName, (event) => deliver(event.payload)) : null,
  );
}

/** Terminal and process output / exit notifications. */
export const streamEvents = tauriChannel<StreamEvent>("runtime://stream");

/** Durable history events (TOOL_CALLED, FILE_CHANGED, …). */
export const auditEvents = tauriChannel<AuditEvent>("runtime://audit");

/** Update checks and downloads (ADR-0019). */
export const updateEvents = tauriChannel<UpdateEvent>("runtime://update");

/** The local models' engine: downloads and its state (ADR-0025). */
export const localEvents = tauriChannel<LocalEvent>("runtime://local");
