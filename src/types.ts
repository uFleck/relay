/* View model shared by the main window and the hover. Rust owns the v2 data; the
   retired v1 proposal, episode, job and claim shapes are gone with the intake pipeline. */

export type HoverEdge = "left" | "right";

/** A local clone with a GitHub `origin`; `projectId` is set when a project already uses the path. */
export interface RepoCandidate {
  path: string;
  repo: { owner: string; name: string };
  projectId: string | null;
}
