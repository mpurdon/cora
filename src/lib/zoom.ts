/** UI zoom via ⌘/Ctrl +, −, and 0 to reset. CSS zoom on the root scales the
 *  whole layout (the app is sized in px throughout). Persisted per window —
 *  the callout and main window want different sizes. */
const MIN = 0.7;
const MAX = 1.6;
const STEP = 0.1;

const clamp = (z: number) => Math.min(MAX, Math.max(MIN, Math.round(z * 10) / 10));

const apply = (z: number) =>
  document.documentElement.style.setProperty("zoom", String(z));

export function initZoom(storageKey: string) {
  let zoom = clamp(Number(localStorage.getItem(storageKey)) || 1);
  apply(zoom);
  window.addEventListener("keydown", (e) => {
    if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
    if (e.key === "=" || e.key === "+") zoom = clamp(zoom + STEP);
    else if (e.key === "-" || e.key === "_") zoom = clamp(zoom - STEP);
    else if (e.key === "0") zoom = 1;
    else return;
    e.preventDefault();
    apply(zoom);
    localStorage.setItem(storageKey, String(zoom));
  });
}
