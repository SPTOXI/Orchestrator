import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";

const root = document.getElementById("root");
if (!root) throw new Error("#root element not found");

// No <StrictMode>: its double-invoked effects would open duplicate terminals
// and processes (real OS resources) in development.
createRoot(root).render(<App />);
