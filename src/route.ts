/* Routes the main window, the tray and the hover pass around. Both windows read them here.
   The three v2 views are Notebook, Tasks and Settings; retired v1 paths stay parseable and
   land on Notebook so a stale link never dead-ends. A reload keeps its logical route in the
   main window's query string, because the WebView always loads `index.html`. */

export type View = "notebook" | "tasks" | "settings";

const VIEWS: readonly View[] = ["notebook", "tasks", "settings"];

/** Recovery destinations linked from Tasks and References. */
export type ProviderKey = "github" | "clickup" | "slack" | "opencode" | "t3";

export const PROVIDER_KEYS: readonly ProviderKey[] = ["github", "clickup", "slack", "opencode", "t3"];

export interface Route {
  view: View;
  /** A selected Task, always addressed from the unfiltered workspace. */
  task: string | null;
  /** A provider to bring into view when Settings opens. */
  provider: ProviderKey | null;
}

export function parseRoute(route: string): Route {
  const url = new URL(route, "http://relay.local");
  const name = url.pathname.replace(/^\/+/, "").split("/")[0];
  const q = url.searchParams;
  const known = VIEWS.includes(name as View);
  // A retired link that still names a Task must open its sheet, not strand the id on Notebook.
  const view: View = known ? (name as View) : q.has("task") ? "tasks" : "notebook";
  const provider = q.get("provider");
  return {
    view,
    task: q.get("task") || null,
    provider:
      view === "settings" && provider && PROVIDER_KEYS.includes(provider as ProviderKey)
        ? (provider as ProviderKey)
        : null,
  };
}

function query(r: Route) {
  const q = new URLSearchParams();
  if (r.task) q.set("task", r.task);
  if (r.provider) q.set("provider", r.provider);
  return q;
}

export function formatRoute(r: Route) {
  const q = query(r).toString();
  return `/${r.view}${q ? `?${q}` : ""}`;
}

export const notebookRoute = () => formatRoute({ view: "notebook", task: null, provider: null });

export const taskRoute = (task: string) =>
  formatRoute({ view: "tasks", task, provider: null });

export const settingsRoute = (provider?: ProviderKey) =>
  formatRoute({ view: "settings", task: null, provider: provider ?? null });

/* The main window keeps its route in its own query string (`?view=tasks&task=…`). */

export function readLocation(): Route {
  const q = new URLSearchParams(window.location.search);
  const view = q.get("view") ?? "notebook";
  q.delete("view");
  const rest = q.toString();
  return parseRoute(`/${view}${rest ? `?${rest}` : ""}`);
}

export function writeLocation(r: Route) {
  const q = query(r);
  if (r.view !== "notebook") q.set("view", r.view);
  const s = q.toString();
  window.history.replaceState(null, "", s ? `?${s}` : window.location.pathname);
}
