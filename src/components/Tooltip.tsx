import { useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

/** Delay before showing, so sweeping the pointer across a list doesn't flash a
 *  card per row. Shorter than the browser's ~1s, since these are informational
 *  rather than a last resort. */
const SHOW_DELAY_MS = 180;
const EDGE_GAP = 8;
/** How far the caret's tip sits from the card's edge — it can't slide closer
 *  than the corner radius without poking out of the rounded corner. */
const CARET_INSET = 14;

/** Rich tooltip bodies, keyed by the element that owns them. Thunks, not
 *  elements: at most one card is on screen, so building the markup for all 150
 *  feed rows on every render is 149 subtrees thrown away. A WeakMap, so an
 *  unmounted trigger takes its content with it. Plain string tooltips don't
 *  come through here at all — they live in the `data-tip` attribute. */
const richContent = new WeakMap<Element, () => ReactNode>();

const SELECTOR = "[data-tip],[data-tip-rich]";

/** One string, both jobs. `title` used to be the tooltip *and* the accessible
 *  name; `data-tip` only does the first, so spread this rather than writing the
 *  text out twice and letting the two drift. */
export function tip(text: string) {
  return { "data-tip": text, "aria-label": text } as const;
}

/** Rich content for an element you can't wrap — a flex child, a row whose
 *  layout a span would disturb. Spread the result onto the element:
 *  `<button {...useRichTip(() => <…/>)}>`. The thunk runs at hover time, so
 *  nothing is built for the rows nobody points at. */
export function useRichTip<T extends HTMLElement>(render: () => ReactNode) {
  const ref = useRef<T>(null);
  // The thunk closes over this render's props; the registration is stable.
  const latest = useRef(render);
  latest.current = render;
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    richContent.set(el, () => latest.current());
    return () => void richContent.delete(el);
  }, []);
  return { ref, "data-tip-rich": "" } as const;
}

/** Wrap an element whose tooltip needs markup rather than a string. Everything
 *  else — the ~99 one-liners — just sets `data-tip="…"` and is picked up by
 *  TooltipLayer's delegated listener, with no wrapper element at all. */
export function Tooltip({
  content,
  children,
  className,
}: {
  content: () => ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <span {...useRichTip<HTMLSpanElement>(content)} className={className}>
      {children}
    </span>
  );
}

type Shown = {
  content: ReactNode;
  x: number;
  y: number;
  /** Caret offset from the card's left edge — kept pointing at the trigger even
   *  when the card has been pushed sideways to stay on screen. */
  caret: number;
  /** Card sits above the trigger, so the caret points down. */
  flipped: boolean;
  /** Measured against the viewport. Until then the card is rendered invisibly
   *  at its first guess, so it has a size to measure. */
  placed: boolean;
};

/** The single tooltip surface for a window. Mount once, near the root.
 *
 *  Delegated rather than per-element: at ~100 tooltips, a wrapper component
 *  around each would inject a span into 100 layouts — flex rows, gutters,
 *  absolutely-positioned things — and every one is a chance to shift something.
 *  A `data-tip` attribute changes nothing about the DOM. */
export function TooltipLayer() {
  const cardRef = useRef<HTMLDivElement>(null);
  const anchorRef = useRef<Element | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const [shown, setShown] = useState<Shown | null>(null);

  useEffect(() => {
    const hide = () => {
      window.clearTimeout(timer.current);
      anchorRef.current = null;
      setShown(null);
    };

    const onOver = (e: PointerEvent) => {
      const el = (e.target as Element | null)?.closest?.(SELECTOR) ?? null;
      if (!el || el === anchorRef.current) return;
      hide();
      anchorRef.current = el;
      timer.current = window.setTimeout(() => {
        const content = richContent.get(el)?.() ?? el.getAttribute("data-tip");
        if (!content) return;
        const r = el.getBoundingClientRect();
        // First pass: below the trigger, left-aligned. The measuring effect
        // corrects for the viewport once the card has a size.
        setShown({
          content,
          x: r.left,
          y: r.bottom + 8,
          caret: CARET_INSET,
          flipped: false,
          placed: false,
        });
      }, SHOW_DELAY_MS);
    };

    const onOut = (e: PointerEvent) => {
      const el = (e.target as Element | null)?.closest?.(SELECTOR);
      if (el && el === anchorRef.current) hide();
    };

    document.addEventListener("pointerover", onOver);
    document.addEventListener("pointerout", onOut);
    // A tooltip that outlives a click is noise once you've acted. Scrolling
    // moves the trigger out from under a fixed card, so that hides too —
    // capture phase, to catch the rail's and the diff's own scrollports.
    document.addEventListener("pointerdown", hide);
    window.addEventListener("scroll", hide, true);
    window.addEventListener("blur", hide);
    return () => {
      window.clearTimeout(timer.current);
      document.removeEventListener("pointerover", onOver);
      document.removeEventListener("pointerout", onOut);
      document.removeEventListener("pointerdown", hide);
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("blur", hide);
    };
  }, []);

  // Measure after paint: the card's own size decides whether it flips above the
  // trigger or shifts sideways, and that isn't knowable until it has rendered.
  useEffect(() => {
    if (!shown || shown.placed || !cardRef.current || !anchorRef.current) return;
    const card = cardRef.current.getBoundingClientRect();
    const anchor = anchorRef.current.getBoundingClientRect();
    let { x, y } = shown;
    let flipped = false;
    if (x + card.width > window.innerWidth - EDGE_GAP) {
      x = Math.max(EDGE_GAP, window.innerWidth - card.width - EDGE_GAP);
    }
    if (y + card.height > window.innerHeight - EDGE_GAP) {
      // Flip above, but never past the top edge — a card taller than the space
      // above would otherwise render with its head off-screen.
      y = Math.max(EDGE_GAP, anchor.top - card.height - 8);
      flipped = anchor.top - card.height - 8 >= EDGE_GAP;
    }
    const caret = Math.min(
      Math.max(anchor.left + anchor.width / 2 - x, CARET_INSET),
      Math.max(CARET_INSET, card.width - CARET_INSET),
    );
    setShown({ ...shown, x, y, caret, flipped, placed: true });
  }, [shown]);

  if (!shown) return null;
  return createPortal(
    <div
      ref={cardRef}
      role="tooltip"
      className={`tooltip-card${shown.flipped ? " flipped" : ""}${shown.placed ? " placed" : ""}`}
      style={
        {
          left: shown.x,
          top: shown.y,
          "--caret-x": `${shown.caret}px`,
        } as React.CSSProperties
      }
    >
      {shown.content}
    </div>,
    document.body,
  );
}
