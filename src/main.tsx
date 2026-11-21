import React from "react";
import ReactDOM from "react-dom/client";
import "./styles/tokens.css";
import "./styles/layout-tokens.css";
import "./styles/base.css";
import "./styles/app.css";
import "./styles/controls.css";
import "./styles/composer.css";
import "./styles/workspace.css";
import "./styles/references.css";
import "./styles/briefing.css";
import "./styles/progress.css";
import App from "./App";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
