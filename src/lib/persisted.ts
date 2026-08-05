import { useCallback, useState } from "react";

/** A boolean persisted to localStorage as "0" | "1" — the shape every rail and
 *  diff toggle shares. Returns the value and a toggler, so call sites never
 *  re-derive the "write the next value from the previous one" inversion.
 *  `initial` is used only when the key has never been written. */
export function usePersistedFlag(key: string, initial = false) {
  const [on, setOn] = useState(() => {
    const raw = localStorage.getItem(key);
    return raw == null ? initial : raw === "1";
  });
  const toggle = useCallback(() => {
    setOn((prev) => {
      localStorage.setItem(key, prev ? "0" : "1");
      return !prev;
    });
  }, [key]);
  return [on, toggle] as const;
}
