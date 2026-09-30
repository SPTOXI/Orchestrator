// The authorization strip (ADR-0016): while an AI waits for the user, the
// oldest request shows under the top bar, on every screen, with the three
// answers. The Autonomy tab has the full list and the details.

import { useState } from "react";
import { requester } from "../lib/autonomy";
import { errorMessage } from "../lib/runtime";
import type { ApprovalAnswer, ApprovalView } from "../lib/types";

interface Props {
  pending: ApprovalView[];
  onAnswer: (id: string, answer: ApprovalAnswer, note?: string | null) => Promise<void>;
  onOpen: () => void;
}

export function ApprovalBar({ pending, onAnswer, onOpen }: Props) {
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const request = pending[0];
  if (!request) return null;

  const answer = async (value: ApprovalAnswer) => {
    setBusy(true);
    setFailure(null);
    try {
      await onAnswer(request.id, value);
    } catch (err) {
      setFailure(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="approval-bar" role="alert">
      <div className="grow approval-text">
        <span className="approval-who">{requester(request)} pede:</span>{" "}
        <strong className="approval-what">{request.summary}</strong>
        <div className="meta ellipsis" title={request.reason}>
          {request.reason}
          {failure && <span className="error-text"> · {failure}</span>}
        </div>
      </div>
      <button className="button small primary" disabled={busy} onClick={() => void answer("approve")}>
        Permitir
      </button>
      <button
        className="button small"
        disabled={busy}
        title="Não perguntar de novo nesta sessão pelo mesmo tipo de chamada"
        onClick={() => void answer("approveSession")}
      >
        Nesta sessão
      </button>
      <button className="button small danger" disabled={busy} onClick={() => void answer("deny")}>
        Negar
      </button>
      <button className="button small" onClick={onOpen}>
        {pending.length > 1 ? `Ver todos (${pending.length})` : "Detalhes"}
      </button>
    </div>
  );
}
