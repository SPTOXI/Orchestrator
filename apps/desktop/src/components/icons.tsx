// Minimal inline icon set (stroke icons, 24x24 grid, currentColor).

import type { ReactNode } from "react";

function Icon({ children, size = 20 }: { children: ReactNode; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.7}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

export const FolderIcon = () => (
  <Icon>
    <path d="M3 6.5A1.5 1.5 0 0 1 4.5 5h4l2 2h9A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5z" />
  </Icon>
);

export const ProvidersIcon = () => (
  <Icon>
    <rect x="6" y="6" width="12" height="12" rx="2" />
    <path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4" />
  </Icon>
);

export const TasksIcon = () => (
  <Icon>
    <path d="M9 6h11M9 12h11M9 18h11" />
    <path d="m3.5 6 1 1 2-2M3.5 12l1 1 2-2M3.5 18l1 1 2-2" />
  </Icon>
);

export const AgentsIcon = () => (
  <Icon>
    <rect x="4" y="8" width="16" height="11" rx="3" />
    <path d="M12 8V4M9 13h.01M15 13h.01" />
    <circle cx="12" cy="3.5" r="1" />
  </Icon>
);

export const TerminalIcon = () => (
  <Icon>
    <rect x="3" y="4" width="18" height="16" rx="2" />
    <path d="m7 9 3 3-3 3M12 15h5" />
  </Icon>
);

export const GitIcon = () => (
  <Icon>
    <circle cx="6" cy="5" r="2" />
    <circle cx="6" cy="19" r="2" />
    <circle cx="18" cy="8" r="2" />
    <path d="M6 7v10M18 10c0 4-6 3-11.5 7" />
  </Icon>
);

export const MemoryIcon = () => (
  <Icon>
    <ellipse cx="12" cy="5.5" rx="7" ry="2.5" />
    <path d="M5 5.5v13c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5v-13M5 12c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5" />
  </Icon>
);

export const HistoryIcon = () => (
  <Icon>
    <path d="M3 12a9 9 0 1 0 3-6.7L3 8" />
    <path d="M3 3v5h5M12 7v5l3 2" />
  </Icon>
);

export const FileIcon = () => (
  <Icon size={16}>
    <path d="M14 3H6.5A1.5 1.5 0 0 0 5 4.5v15A1.5 1.5 0 0 0 6.5 21h11a1.5 1.5 0 0 0 1.5-1.5V8z" />
    <path d="M14 3v5h5" />
  </Icon>
);

export const ChevronIcon = ({ open }: { open: boolean }) => (
  <Icon size={14}>
    <path d={open ? "m6 9 6 6 6-6" : "m9 6 6 6-6 6"} />
  </Icon>
);

export const PlusIcon = () => (
  <Icon size={16}>
    <path d="M12 5v14M5 12h14" />
  </Icon>
);

export const RefreshIcon = () => (
  <Icon size={16}>
    <path d="M20 11a8 8 0 1 0-2.3 5.7M20 5v6h-6" />
  </Icon>
);

export const UpIcon = () => (
  <Icon size={16}>
    <path d="M12 19V5M6 11l6-6 6 6" />
  </Icon>
);

export const CloseIcon = () => (
  <Icon size={14}>
    <path d="M6 6l12 12M18 6 6 18" />
  </Icon>
);

export const EditIcon = () => (
  <Icon size={14}>
    <path d="M4 20h4L19 9l-4-4L4 16z" />
  </Icon>
);

export const TrashIcon = () => (
  <Icon size={14}>
    <path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3" />
  </Icon>
);

export const PlayIcon = () => (
  <Icon size={14}>
    <path d="M7 5v14l11-7z" />
  </Icon>
);

export const StopIcon = () => (
  <Icon size={14}>
    <rect x="6" y="6" width="12" height="12" rx="1" />
  </Icon>
);
