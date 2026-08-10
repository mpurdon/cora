import { useCallback, useState } from "react";

/** A string persisted to localStorage — the shape every remembered rail choice
 *  shares (group mode, sort mode, open panels). `initial` is used only when the
 *  key has never been written. */
export function usePersisted<T extends string>(key: string, initial: T) {
  const [value, setValue] = useState<T>(() => (localStorage.getItem(key) as T) ?? initial);
  const set = useCallback(
    (v: T) => {
      setValue(v);
      localStorage.setItem(key, v);
    },
    [key],
  );
  return [value, set] as const;
}

/** The boolean case, stored as "0" | "1". Returns the value, a toggler and a
 *  setter, so call sites never re-derive the string conversion or the
 *  "write the next value from the previous one" inversion. */
export function usePersistedFlag(key: string, initial = false) {
  const [raw, setRaw] = usePersisted<"0" | "1">(key, initial ? "1" : "0");
  const on = raw === "1";
  const set = useCallback((next: boolean) => setRaw(next ? "1" : "0"), [setRaw]);
  const toggle = useCallback(() => set(!on), [set, on]);
  return [on, toggle, set] as const;
}
