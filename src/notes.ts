import type { Note } from "./ipc";

/* The T3 handoff: a note becomes a prompt, optionally led by a slash command. Relay copies it and opens
   T3 Code; the user sends it there. Relay never starts an agent. */

export type PromptTemplate = "" | "/ship" | "/orchestrate" | "/draft-issues";

export const PROMPT_TEMPLATES: { value: PromptTemplate; label: string }[] = [
  { value: "", label: "Plain prompt" },
  { value: "/ship", label: "/ship" },
  { value: "/orchestrate", label: "/orchestrate" },
  { value: "/draft-issues", label: "/draft-issues" },
];

/** `<template> <title>` on the first line, a blank line, then the body. Empty parts drop out. */
export function composePrompt(note: Pick<Note, "title" | "body">, template: PromptTemplate) {
  const head = [template, note.title.trim()].filter(Boolean).join(" ");
  return [head, note.body.trim()].filter(Boolean).join("\n\n");
}

/** The list label: the title, else the body's first line. */
export function noteLabel(note: Pick<Note, "title" | "body">) {
  return note.title.trim() || note.body.split("\n").map((l) => l.trim()).find(Boolean) || "Untitled";
}

/** Pinned first, then the most recently edited, the order `notes_list` returns. */
export const sortNotes = (notes: Note[]) =>
  [...notes].sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.updatedAt.localeCompare(a.updatedAt));
