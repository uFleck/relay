/* Release bodies are GitHub Markdown. The update tooltip shows them as plain lines: list
   items become bullets, headings become small labels, everything else is a sentence. */

export interface ChangelogLine {
  kind: "head" | "item" | "text";
  text: string;
}

const inline = (text: string) =>
  text
    .replace(/!\[[^\]]*\]\([^)]*\)/g, "")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/(\*\*|__|`)/g, "")
    .trim();

export function changelogLines(body: string): ChangelogLine[] {
  const lines: ChangelogLine[] = [];
  for (const raw of body.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || /^(-{3,}|<!--.*-->)$/.test(line)) continue;
    const heading = /^#{1,6}\s+(.*)$/.exec(line);
    const item = /^(?:[-*+]|\d+\.)\s+(.*)$/.exec(line);
    const text = inline(heading?.[1] ?? item?.[1] ?? line);
    if (!text) continue;
    lines.push({ kind: heading ? "head" : item ? "item" : "text", text });
  }
  return lines;
}
