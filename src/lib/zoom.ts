/** UI zoom via ⌘/Ctrl +, −, and 0 to reset. CSS zoom on the root scales the
 *  whole layout (the app is sized in px throughout). Persisted per window —
 *  the callout and main window want different sizes. */
const MIN = 0.7;
const MAX = 1.6;
const STEP = 0.1;

const clamp = (z: number) => Math.min(MAX, Math.max(MIN, Math.round(z * 10) / 10));

const apply = (z: number) =>
  document.documentElement.style.setProperty("zoom", String(z));

/** Visual px per layout px at the root — 1.5 at 150% zoom.
 *
 *  Two coordinate spaces coexist under CSS zoom. `getBoundingClientRect`,
 *  `clientX`/`clientY` and `window.innerWidth` all speak *visual* px: where a
 *  thing is on the screen. But `left`/`top` on a `position: fixed` element
 *  inside the zoomed root are *layout* px, and get multiplied by the zoom again
 *  on the way to the screen. Copy a measured anchor straight into `left` and
 *  the popup lands at anchor × zoom — barely off near the top-left corner,
 *  hundreds of px off at the far side of a wide window. Anything positioned
 *  from a measurement has to divide by this first.
 *
 *  Measured rather than read back from the style, so it stays right whichever
 *  convention the webview uses: WebKit and Chromium ≥ 128 report visual px
 *  (this measures the zoom); older Chromium reported layout px, where the two
 *  spaces coincide and this measures 1. */
export function visualScale(): number {
  const root = document.documentElement;
  return root.offsetWidth > 0 ? root.getBoundingClientRect().width / root.offsetWidth : 1;
}

export type LayoutBox = {
  left: number;
  top: number;
  right: number;
  bottom: number;
  width: number;
  height: number;
};

/** An element's bounding box in layout px — the numbers a fixed element's
 *  `left`/`top` can be set from directly. */
export function layoutRect(el: Element): LayoutBox {
  const s = visualScale();
  const r = el.getBoundingClientRect();
  return {
    left: r.left / s,
    top: r.top / s,
    right: r.right / s,
    bottom: r.bottom / s,
    width: r.width / s,
    height: r.height / s,
  };
}

/** A pointer position (`clientX`/`clientY`) in layout px. */
export function layoutPoint(clientX: number, clientY: number) {
  const s = visualScale();
  return { x: clientX / s, y: clientY / s };
}

/** The viewport in layout px — the box a fixed element has to stay inside. */
export function layoutViewport() {
  const s = visualScale();
  return { width: window.innerWidth / s, height: window.innerHeight / s };
}

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
