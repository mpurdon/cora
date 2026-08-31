import { useRef } from "react";

export interface PrioritySelectorProps<T> {
  levels: T[];
  value: T;
  onChange: (level: T) => void;
  getLabel: (level: T) => string;
  getIcon: (level: T) => React.ReactNode;
  groupLabel: string;
}

/** Inline icon radiogroup, generic over whatever level type the caller has
 *  (repo priority, PR priority, …). Follows the WAI-ARIA radio group pattern:
 *  arrow keys both move focus and select (there's only ever one checked item
 *  in a radiogroup, so the two can't diverge), Home/End jump to the ends, and
 *  only the checked item sits in the tab order. */
export function PrioritySelector<T>({
  levels,
  value,
  onChange,
  getLabel,
  getIcon,
  groupLabel,
}: PrioritySelectorProps<T>) {
  const itemRefs = useRef<(HTMLSpanElement | null)[]>([]);

  const select = (index: number) => {
    onChange(levels[index]);
    itemRefs.current[index]?.focus();
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLSpanElement>, index: number) => {
    switch (e.key) {
      case "ArrowRight":
      case "ArrowDown":
        e.preventDefault();
        select((index + 1) % levels.length);
        break;
      case "ArrowLeft":
      case "ArrowUp":
        e.preventDefault();
        select((index - 1 + levels.length) % levels.length);
        break;
      case "Home":
        e.preventDefault();
        select(0);
        break;
      case "End":
        e.preventDefault();
        select(levels.length - 1);
        break;
      case "Enter":
      case " ":
        e.preventDefault();
        select(index);
        break;
      default:
        break;
    }
  };

  return (
    <div role="radiogroup" aria-label={groupLabel} className="priority-selector">
      {levels.map((level, index) => {
        const selected = level === value;
        const label = getLabel(level);
        return (
          <span
            key={String(level)}
            ref={(el) => {
              itemRefs.current[index] = el;
            }}
            role="radio"
            aria-checked={selected}
            aria-label={label}
            title={label}
            tabIndex={selected ? 0 : -1}
            className={`priority-selector-item${selected ? " selected" : ""}`}
            onClick={() => select(index)}
            onKeyDown={(e) => handleKeyDown(e, index)}
          >
            {getIcon(level)}
          </span>
        );
      })}
    </div>
  );
}
