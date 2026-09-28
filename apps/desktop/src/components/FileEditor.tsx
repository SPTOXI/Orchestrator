// Main area: plain-text editor over filesystem.read / filesystem.write.

import { type KeyboardEvent, useCallback, useEffect, useRef, useState } from "react";
import { auditEvents } from "../lib/events";
import { type Eol, detectEol, formatBytes, isInside, toLf, withEol } from "../lib/format";
import { errorMessage, fsApi } from "../lib/runtime";
import type { Encoding } from "../lib/types";

/** Files larger than this open read-only (first bytes only). */
const MAX_EDITABLE_BYTES = 2 * 1024 * 1024;

interface Props {
  path: string;
  active: boolean;
  onDirtyChange: (path: string, dirty: boolean) => void;
}

interface Loaded {
  /** Content as shown in the editor (LF line endings). */
  original: string;
  /** Line ending of the file on disk, restored on save. */
  eol: Eol;
  encoding: Encoding;
  size: number;
  truncated: boolean;
}

export function FileEditor({ path, active, onDirtyChange }: Props) {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [content, setContent] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const textRef = useRef<HTMLTextAreaElement>(null);

  const dirty = loaded !== null && content !== loaded.original;
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;

  const load = useCallback(async () => {
    try {
      const out = await fsApi.read(path, { maxBytes: MAX_EDITABLE_BYTES });
      // <textarea> normalizes CRLF to LF; compare and edit in LF and restore
      // the original line ending when saving.
      const text = out.encoding === "utf8" ? toLf(out.content) : out.content;
      setLoaded({
        original: text,
        eol: detectEol(out.content),
        encoding: out.encoding,
        size: out.size,
        truncated: out.truncated,
      });
      setContent(text);
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [path]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => onDirtyChange(path, dirty), [path, dirty, onDirtyChange]);

  // Follow changes made by others (terminal, processes, future agents).
  useEffect(
    () =>
      auditEvents.subscribe((event) => {
        if (event.kind !== "FILE_CHANGED") return;
        const data = event.data as { change?: string; path?: string; from?: string; to?: string };
        if (data.change === "deleted" && data.path && isInside(path, data.path)) {
          setNotice("O arquivo foi excluído do disco.");
        } else if (data.change === "moved" && data.from && isInside(path, data.from)) {
          setNotice(`O arquivo foi movido (${data.from} → ${data.to}).`);
        } else if ((data.path === path || data.to === path) && !dirtyRef.current) {
          setNotice(null);
          void load();
        }
      }),
    [path, load],
  );

  useEffect(() => {
    if (active) textRef.current?.focus();
  }, [active]);

  const readOnly = !loaded || loaded.encoding !== "utf8" || loaded.truncated;

  const save = async () => {
    if (!loaded || readOnly || saving) return;
    setSaving(true);
    try {
      const text = withEol(content, loaded.eol);
      await fsApi.write(path, text);
      setLoaded((l) => (l ? { ...l, original: content, size: new TextEncoder().encode(text).length } : l));
      setNotice(null);
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
      e.preventDefault();
      void save();
    } else if (e.key === "Tab" && !e.shiftKey && !readOnly) {
      e.preventDefault();
      const el = e.currentTarget;
      el.setRangeText("\t", el.selectionStart, el.selectionEnd, "end");
      setContent(el.value);
    }
  };

  return (
    <div className="editor" hidden={!active}>
      <div className="editor-toolbar">
        <span className="mono path" title={path}>
          {path}
        </span>
        {loaded && <span className="meta">{formatBytes(loaded.size)}</span>}
        {dirty && <span className="badge warn">não salvo</span>}
        <button className="button small primary" onClick={() => void save()} disabled={readOnly || !dirty || saving}>
          {saving ? "Salvando…" : "Salvar"} <kbd>Ctrl+S</kbd>
        </button>
      </div>
      {notice && <div className="inline-notice">{notice}</div>}
      {error && <div className="inline-error">{error}</div>}
      {loaded?.encoding === "base64" ? (
        <div className="empty">Arquivo binário ({formatBytes(loaded.size)}) — não pode ser editado como texto.</div>
      ) : (
        <>
          {loaded?.truncated && (
            <div className="inline-notice">
              Arquivo grande: exibindo os primeiros {formatBytes(MAX_EDITABLE_BYTES)} em modo somente leitura.
            </div>
          )}
          <textarea
            ref={textRef}
            className="editor-text mono"
            value={content}
            onChange={(e) => setContent(e.target.value)}
            onKeyDown={onKeyDown}
            readOnly={readOnly}
            spellCheck={false}
            wrap="off"
          />
        </>
      )}
    </div>
  );
}
