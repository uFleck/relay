import { useEffect } from "react";
import { VIM_HELP } from "../vim";

/* The cheat sheet `?` opens. Esc, `?` or a click outside closes it. */
export function VimHelp({ onClose }: { onClose: () => void }) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" || event.key === "?") {
        event.preventDefault();
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div
      className="search-shade"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <section className="vim-help" role="dialog" aria-modal="true" aria-label="Vim keys">
        <header className="sm-head">
          <b>Vim keys</b>
          <span className="push" />
          <span className="meta">esc to close</span>
        </header>
        <dl>
          {VIM_HELP.map((row) => (
            <div key={row.keys}>
              <dt>
                {row.keys.split(" / ").map((key, index) => (
                  <span key={key}>
                    {index > 0 && " / "}
                    <kbd>{key}</kbd>
                  </span>
                ))}
              </dt>
              <dd>{row.does}</dd>
            </div>
          ))}
        </dl>
      </section>
    </div>
  );
}
