import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { call, inTauri, onEvent } from "./ipc";

/* The summon chord comes from a config file in app data. Both windows read the same cache
   and subscribe to changes, so the rail hint and the Dock hint never drift apart. */

export interface Keybinds {
  summon: string;
}

export const DEFAULT_KEYBINDS: Keybinds = { summon: "ctrl+alt+r" };

let cached: Keybinds = DEFAULT_KEYBINDS;

export const getKeybinds = () =>
  call<Keybinds>("v2_keybinds_get", undefined, () => DEFAULT_KEYBINDS);

export const setKeybinds = async (binds: Keybinds): Promise<Keybinds> => {
  if (!inTauri) throw new Error("Keybinds can only be changed in the Relay desktop app.");
  const saved = await invoke<Keybinds>("v2_keybinds_set", { binds });
  cached = saved;
  return saved;
};

export const onKeybindsChange = (handler: (binds: Keybinds) => void) =>
  onEvent("relay://keybinds-change", (binds) => {
    cached = binds;
    handler(binds);
  });

export const summonChord = () => cached.summon;

/** Reads the config once and follows later edits. */
export function useKeybinds(): Keybinds {
  const [binds, setBinds] = useState<Keybinds>(cached);
  useEffect(() => {
    let gone = false;
    void getKeybinds().then((value) => {
      if (gone) return;
      cached = value;
      setBinds(value);
    });
    const off = onKeybindsChange(setBinds);
    return () => {
      gone = true;
      off();
    };
  }, []);
  return binds;
}
