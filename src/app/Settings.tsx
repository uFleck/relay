import { useEffect } from "react";
import type { ProviderKey } from "../route";
import { UpdateActions } from "./Views";
import type { UpdateState } from "../updater";

/* The dedicated v2 Settings entry. #28 owns provider setup and mapping; this transition
   view accepts a recovery destination and keeps the existing updater controls available. */

const PROVIDERS: readonly { key: ProviderKey; label: string; detail: string }[] = [
  { key: "github", label: "GitHub", detail: "Read-only PRs and issues. Credentials stay outside the frontend." },
  { key: "clickup", label: "ClickUp", detail: "Read-only cards and Import scope." },
  { key: "slack", label: "Slack", detail: "Conversation context through headless Haiku." },
  { key: "opencode", label: "OpenCode", detail: "Briefs and Grilling with inexpensive models." },
  { key: "t3", label: "T3 Code", detail: "Dispatch, project registration and thread progress." },
];

interface Props {
  provider: ProviderKey | null;
  version: string;
  update: UpdateState;
}

export function SettingsView({ provider, version, update }: Props) {
  useEffect(() => {
    if (!provider) return;
    document.getElementById(`provider-${provider}`)?.scrollIntoView({ block: "start" });
  }, [provider]);

  return (
    <section className="main settings" aria-label="Settings">
      <header className="head">
        <div>
          <h1>Settings</h1>
          <div className="date meta">Relay {version}</div>
        </div>
      </header>
      <div className="scroll">
        <h2 className="sub" id="settings-updates">
          Updates
        </h2>
        <div className="setrow">
          <span>{update.kind === "error" ? `Update failed: ${update.message}` : `Relay ${version}`}</span>
          <span className="push" />
          <UpdateActions update={update} />
        </div>

        <h2 className="sub">Providers</h2>
        <p className="empty">
          {provider
            ? `Opened at ${provider}. Provider setup lands here in #28.`
            : "Provider setup lands here in #28. Status is shown without exposing credentials."}
        </p>
        {PROVIDERS.map((item) => (
          <section className="setrow provider" id={`provider-${item.key}`} key={item.key}>
            <h3>{item.label}</h3>
            <span className="meta">{item.detail}</span>
          </section>
        ))}
      </div>
    </section>
  );
}
