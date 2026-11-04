import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { Overlay } from "./Overlay";
import "./index.css";

// Desktop app: suppress the browser context menu except in text fields.
document.addEventListener("contextmenu", (e) => {
  const t = e.target as HTMLElement;
  if (!t.closest("input, textarea, [data-native-menu]")) e.preventDefault();
});

const isOverlay = window.location.hash === "#overlay";

createRoot(document.getElementById("root")!).render(<StrictMode>{isOverlay ? <Overlay /> : <App />}</StrictMode>);
