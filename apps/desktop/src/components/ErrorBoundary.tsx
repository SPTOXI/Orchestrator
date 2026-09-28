// Last-resort view for render errors: shows the error instead of a blank
// window and offers a reload. The runtime (terminals, processes) keeps
// running in the Rust process; reloading only rebuilds the UI.

import { Component, type ErrorInfo, type ReactNode } from "react";

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("[orchestrator] UI error", error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="crash">
        <h1>A interface encontrou um erro</h1>
        <p>
          Terminais, processos e o projeto aberto continuam ativos no runtime. Recarregar reconstrói apenas a
          interface.
        </p>
        <pre>{error.stack ?? error.message}</pre>
        <button className="button primary" onClick={() => window.location.reload()}>
          Recarregar interface
        </button>
      </div>
    );
  }
}
