import type { LinkKind, Project } from "../types";
import type { IconName } from "./Icon";

export const linkIcon: Record<LinkKind, IconName> = {
  slack: "slack",
  thread: "thread",
  pr: "pr",
  branch: "branch",
  clickup: "clickup",
  github: "github",
};

export function ProjectChip({ project }: { project: Project | undefined }) {
  if (!project) return null;
  return (
    <span className="proj" style={{ "--pc": `var(${project.colorVar})` } as React.CSSProperties}>
      <i />
      {project.name}
    </span>
  );
}

/** Renders `code` spans from backtick-quoted text. */
export function Prose({ text }: { text: string }) {
  return (
    <>
      {text.split("`").map((part, i) => (i % 2 ? <code key={i}>{part}</code> : part))}
    </>
  );
}
