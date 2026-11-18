/* Routes are the strings Rust and the hover pass around: `/review`, `/settings`,
   `/today?task=<id>&focus=changes`, `/review?proposal=<id>`, `/inbox?episode=<id>`. Both windows read them here. */

export type View = "today" | "inbox" | "projects" | "agents" | "review" | "settings";

const VIEWS: readonly View[] = ["today", "inbox", "projects", "agents", "review", "settings"];

export interface Route {
  view: View;
  task: string | null;
  focus: string | null;
  proposal: string | null;
  /** A Slack episode whose transcript sheet is open. */
  episode: string | null;
}

export function parseRoute(route: string): Route {
  const url = new URL(route, "http://relay.local");
  const name = url.pathname.replace(/^\/+/, "").split("/")[0];
  const q = url.searchParams;
  return {
    view: VIEWS.includes(name as View) ? (name as View) : "today",
    task: q.get("task"),
    focus: q.get("focus"),
    proposal: q.get("proposal"),
    episode: q.get("episode"),
  };
}

function query(r: Route) {
  const q = new URLSearchParams();
  if (r.task) q.set("task", r.task);
  if (r.focus) q.set("focus", r.focus);
  if (r.proposal) q.set("proposal", r.proposal);
  if (r.episode) q.set("episode", r.episode);
  return q;
}

export function formatRoute(r: Route) {
  const q = query(r).toString();
  return `/${r.view}${q ? `?${q}` : ""}`;
}

export const taskRoute = (task: string) => formatRoute({ view: "today", task, focus: "changes", proposal: null, episode: null });

export const proposalRoute = (proposal: string) => formatRoute({ view: "review", task: null, focus: null, proposal, episode: null });

export const episodeRoute = (episode: string) => formatRoute({ view: "inbox", task: null, focus: null, proposal: null, episode });

/* The main window keeps its route in its own query string (`?view=review&task=…`), so a reload lands
   in the same place without the webview resolving a path. */

export function readLocation(): Route {
  const q = new URLSearchParams(window.location.search);
  const view = q.get("view") ?? "today";
  q.delete("view");
  const rest = q.toString();
  return parseRoute(`/${view}${rest ? `?${rest}` : ""}`);
}

export function writeLocation(r: Route) {
  const q = query(r);
  if (r.view !== "today") q.set("view", r.view);
  const s = q.toString();
  window.history.replaceState(null, "", s ? `?${s}` : window.location.pathname);
}
