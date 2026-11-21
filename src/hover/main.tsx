import React from "react";
import ReactDOM from "react-dom/client";
import "../styles/tokens.css";
import "../styles/layout-tokens.css";
import "../styles/base.css";
import "./hover.css";
import "../styles/composer.css";
import "../styles/references.css";
import { HoverApp } from "./HoverApp";

// Browser dev: /hover.html shows the needs-you panel on mock data, /hover.html?calm the idle one.
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <HoverApp />
  </React.StrictMode>,
);
