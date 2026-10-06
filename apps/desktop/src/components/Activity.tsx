// "The AI is working" indicators (lib/activity.ts): the line above the
// composer of a session and a running clock for lists and bars.

import { activityLabel, formatElapsed, silence, type SessionActivity } from "../lib/activity";
import { useNow } from "../lib/useSessionActivity";

/** A clock since `since` (ms), ticking by itself. */
export function Elapsed({ since }: { since: number }) {
  const now = useNow(true);
  return <>{formatElapsed(now - since)}</>;
}

interface LineProps {
  activity: SessionActivity;
  providerName: string;
  /** The tool being run waits for the user's authorization. */
  waitingForUser: boolean;
}

export function ActivityLine({ activity, providerName, waitingForUser }: LineProps) {
  const now = useNow(true);
  const quiet = silence(activity, now);
  return (
    <div
      className={`activity-line${waitingForUser ? " waiting" : ""}`}
      role="status"
      title={activity.phase === "retrying" ? (activity.detail ?? undefined) : undefined}
    >
      <span className="spinner" aria-hidden />
      <span className="grow ellipsis">
        <strong>{providerName}</strong> · {activityLabel(activity, waitingForUser)}
      </span>
      {quiet !== null && <span className="warn-text">sem sinal da IA há {formatElapsed(quiet)}</span>}
      <span className="meta mono" title="Tempo deste turno">
        {formatElapsed(now - activity.since)}
      </span>
    </div>
  );
}
