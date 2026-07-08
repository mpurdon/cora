import { useEffect, useRef } from "react";

export interface MenuItem {
  label: string;
  checked?: boolean;
  danger?: boolean;
  onClick: () => void;
}

export interface MenuSection {
  title?: string;
  items: MenuItem[];
}

/** Lightweight custom context menu (native menus can't render in-window). */
export function ContextMenu({
  x,
  y,
  sections,
  onClose,
}: {
  x: number;
  y: number;
  sections: MenuSection[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);

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

  // Keep the menu inside the viewport.
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    if (rect.bottom > window.innerHeight) {
      el.style.top = `${Math.max(8, y - rect.height)}px`;
    }
    if (rect.right > window.innerWidth) {
      el.style.left = `${Math.max(8, x - rect.width)}px`;
    }
  }, [x, y]);

  return (
    <div ref={ref} className="context-menu" style={{ left: x, top: y }} role="menu">
      {sections.map((section, si) => (
        <div key={si} className="menu-section">
          {section.title && <div className="menu-title">{section.title}</div>}
          {section.items.map((item, ii) => (
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
          ))}
        </div>
      ))}
    </div>
  );
}
