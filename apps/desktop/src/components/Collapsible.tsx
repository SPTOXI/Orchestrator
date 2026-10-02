// A sidebar section whose body the user can hide; the choice is kept.

import type { ReactNode } from "react";
import { usePersistentToggle } from "../lib/usePersistentToggle";
import { ChevronIcon } from "./icons";

interface Props {
  /** Storage key for the open/closed state. */
  id: string;
  title: ReactNode;
  /** Shown next to the title while the section is closed. */
  summary?: ReactNode;
  /** Extra controls on the right of the title (stay visible when closed). */
  actions?: ReactNode;
  defaultOpen?: boolean;
  children: ReactNode;
}

export function Collapsible({ id, title, summary, actions, defaultOpen = true, children }: Props) {
  const [open, setOpen] = usePersistentToggle(`section.${id}`, defaultOpen);
  return (
    <div className={`collapsible ${open ? "open" : "closed"}`}>
      <div className="section-title row collapsible-head">
        <button className="collapsible-toggle grow" aria-expanded={open} onClick={() => setOpen(!open)}>
          <ChevronIcon open={open} />
          <span className="collapsible-title">{title}</span>
          {!open && summary && <span className="collapsible-summary">{summary}</span>}
        </button>
        {actions}
      </div>
      {open && children}
    </div>
  );
}
