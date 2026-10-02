// "Run this in a terminal" from anywhere in the UI (installing Ollama,
// logging into a CLI…): the terminal panel opens a new terminal, types the
// command and shows it; the user sees everything and can answer prompts.

type Listener = (command: string) => void;

const listeners = new Set<Listener>();

export const terminalRequests = {
  run(command: string) {
    for (const listener of listeners) listener(command);
  },
  subscribe(listener: Listener): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
};
