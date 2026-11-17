import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";

/**
 * Scaffold placeholder. The real shell (rail, task graph, sheets) is owned by the UI agent.
 * The button is here so the future hover window can summon the main window through the same entry point.
 */
export default function App() {
  const [summoned, setSummoned] = useState(false);

  async function openMainWindow() {
    try {
      await invoke("show_main_window");
    } catch {
      // In a plain browser (pnpm dev) there is no Tauri bridge; the button is still inert-safe.
    }
    setSummoned(true);
  }

  return (
    <main className="scaffold">
      <p className="scaffold__eyebrow">scaffold</p>
      <h1 className="scaffold__title">Relay</h1>
      <p className="scaffold__note">Local-first task graph for AI sessions.</p>
      <button className="scaffold__button" type="button" onClick={openMainWindow}>
        {summoned ? "Main window shown" : "Open main window"}
      </button>
    </main>
  );
}
