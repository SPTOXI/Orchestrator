import { createRoot } from "react-dom/client";
import { App } from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import "./styles.css";

const root = document.getElementById("root");
if (!root) throw new Error("#root element not found");

// No <StrictMode>: its double-invoked effects would open duplicate terminals
// and processes (real OS resources) in development.
createRoot(root).render(
  <ErrorBoundary>
    <App />
  </ErrorBoundary>,
);
