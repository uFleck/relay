import { useEffect, useRef, useState } from "react";

/* Animations key off identity, never off a whole re-render. Only ids that were not in the
   previous snapshot get the entering class, so ordinary typing and unchanged refreshes stay
   quiet. `useEntering` clears the set once the animation has played. */

export function addedKeys(previous: ReadonlySet<string>, current: Iterable<string>): Set<string> {
  const added = new Set<string>();
  for (const key of current) {
    if (!previous.has(key)) added.add(key);
  }
  return added;
}

export function useEntering(keys: Iterable<string>, durationMs = 700): ReadonlySet<string> {
  const signature = [...keys].join("|");
  const [entering, setEntering] = useState<ReadonlySet<string>>(new Set());
  const seen = useRef<Set<string> | null>(null);

  useEffect(() => {
    const current = signature ? signature.split("|") : [];
    if (seen.current === null) {
      // The first snapshot is already on screen; it must not rise in as a new arrival.
      seen.current = new Set(current);
      return;
    }
    const added = addedKeys(seen.current, current);
    seen.current = new Set(current);
    if (added.size === 0) return;
    setEntering(added);
    const timer = window.setTimeout(() => setEntering(new Set()), durationMs);
    return () => window.clearTimeout(timer);
  }, [signature, durationMs]);

  return entering;
}
