// xterm.js view used for interactive terminals and read-only process output.

import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";

const THEME = {
  background: "#0b1016",
  foreground: "#d5dde7",
  cursor: "#3d8bfd",
  selectionBackground: "#264f78",
  black: "#1b2230",
  red: "#f85149",
  green: "#3fb950",
  yellow: "#d29922",
  blue: "#58a6ff",
  magenta: "#bc8cff",
  cyan: "#39c5cf",
  white: "#d5dde7",
  brightBlack: "#6e7781",
  brightRed: "#ff7b72",
  brightGreen: "#56d364",
  brightYellow: "#e3b341",
  brightBlue: "#79c0ff",
  brightMagenta: "#d2a8ff",
  brightCyan: "#56d4dd",
  brightWhite: "#f0f6fc",
};

export interface XTermViewProps {
  /** Re-creates the terminal when it changes (terminal/process id). */
  sourceKey: string;
  active: boolean;
  readOnly?: boolean;
  /** Connects the view to its data source; returns a cleanup function. */
  connect: (term: Terminal) => () => void;
  onInput?: (data: string) => void;
  onResize?: (cols: number, rows: number) => void;
}

function fitSafely(fit: FitAddon) {
  try {
    if (fit.proposeDimensions()) fit.fit();
  } catch {
    // Not attached to a visible element yet.
  }
}

export function XTermView({ sourceKey, active, readOnly = false, connect, onInput, onResize }: XTermViewProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<{ term: Terminal; fit: FitAddon } | null>(null);
  const callbacks = useRef({ connect, onInput, onResize });
  callbacks.current = { connect, onInput, onResize };

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const term = new Terminal({
      theme: THEME,
      fontFamily: '"Cascadia Code", "JetBrains Mono", "Fira Code", Menlo, Consolas, "DejaVu Sans Mono", monospace',
      fontSize: 13,
      lineHeight: 1.15,
      cursorBlink: !readOnly,
      disableStdin: readOnly,
      convertEol: readOnly,
      scrollback: 10_000,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    viewRef.current = { term, fit };

    const disconnect = callbacks.current.connect(term);
    const dataSub = term.onData((data) => callbacks.current.onInput?.(data));
    let resizeTimer: number | undefined;
    const resizeSub = term.onResize(({ cols, rows }) => {
      window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(() => callbacks.current.onResize?.(cols, rows), 60);
    });
    const observer = new ResizeObserver(() => fitSafely(fit));
    observer.observe(host);
    fitSafely(fit);

    return () => {
      observer.disconnect();
      window.clearTimeout(resizeTimer);
      dataSub.dispose();
      resizeSub.dispose();
      disconnect();
      term.dispose();
      viewRef.current = null;
    };
  }, [sourceKey, readOnly]);

  useEffect(() => {
    const view = viewRef.current;
    if (!active || !view) return;
    fitSafely(view.fit);
    if (!readOnly) view.term.focus();
  }, [active, readOnly]);

  return <div className="xterm-host" ref={hostRef} hidden={!active} />;
}
