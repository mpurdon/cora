import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { layoutPoint, layoutRect, layoutViewport } from "../lib/zoom";

const EDGE_GAP = 8;

export interface MenuItem {
  type?: "item";
  label: string;
  checked?: boolean;
  danger?: boolean;
  onClick: () => void;
}

export interface MenuCustomRow {
  type: "custom";
  key: string;
  render: () => React.ReactNode;
}

export interface MenuSection {
  title?: string;
  items: (MenuItem | MenuCustomRow)[];
}

/** Lightweight custom context menu (native menus can't render in-window).
 *
 *  `x`/`y` are client px — a pointer's `clientX`/`clientY`, or an edge of the
 *  opener's `getBoundingClientRect()` — and the menu does the conversion into
 *  the layout px its fixed position is set in, so callers never have to know
 *  the app is zoomed. `align` says which edge of the menu sits at `x`:
 *  "right" for a button at the far side of a row, so the menu grows back
 *  across the content it belongs to rather than off into whatever is beside it. */
export function ContextMenu({
  x,
  y,
  align = "left",
  sections,
  onClose,
}: {
  x: number;
  y: number;
  align?: "left" | "right";
  sections: MenuSection[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onDown);
    };
  }, [onClose]);

  // Place before first paint: the menu needs its own size to know which way to
  // open, and until it's placed it's rendered hidden at the origin so the size
  // is there to measure. All in layout px — see layoutRect for why the raw
  // client numbers can't be written into `left`/`top` as they are.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const anchor = layoutPoint(x, y);
    const { width, height } = layoutRect(el);
    const viewport = layoutViewport();
    let left = align === "right" ? anchor.x - width : anchor.x;
    let top = anchor.y;
    // Flip to the anchor's other side when the preferred one runs off-screen,
    // and never past the top-left corner either way.
    if (left + width > viewport.width - EDGE_GAP) left = anchor.x - width;
    if (left < EDGE_GAP) left = Math.max(EDGE_GAP, Math.min(anchor.x, viewport.width - width - EDGE_GAP));
    if (top + height > viewport.height - EDGE_GAP) top = Math.max(EDGE_GAP, anchor.y - height);
    setPos({ left, top });
  }, [x, y, align]);

  return (
    <div
      ref={ref}
      className="context-menu"
      style={pos ?? { left: 0, top: 0, visibility: "hidden" }}
      role="menu"
    >
      {sections.map((section, si) => (
        <div key={si} className="menu-section">
          {section.title && <div className="menu-title">{section.title}</div>}
          {section.items.map((item, ii) => {
            switch (item.type) {
              case "custom":
                return <div key={item.key}>{item.render()}</div>;
              case "item":
              case undefined:
                return (
                  <button
                    key={ii}
                    role="menuitem"
                    className={`menu-item${item.danger ? " danger" : ""}`}
                    onClick={() => {
                      item.onClick();
                      onClose();
                    }}
                  >
                    <span className="menu-check">{item.checked ? "✓" : ""}</span>
                    {item.label}
                  </button>
                );
              default:
                return item satisfies never;
            }
          })}
        </div>
      ))}
    </div>
  );
}
