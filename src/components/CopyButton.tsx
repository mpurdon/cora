import { useState } from "react";
import { IconCheck, IconClipboard } from "./icons";
import { tip } from "./Tooltip";

/** Clipboard button with inline feedback, shared by every "copy this" affordance
 *  — a file's patch, a whole file, a PR link.
 *
 *  `text` may be a thunk, and usually should be: the diff view has one of these
 *  per file, and cutting every file's patch up front to serve the one button
 *  someone eventually presses is a second copy of the whole diff. A plain `null`
 *  means there is nothing to copy *yet* (content still fetching) and disables
 *  the button rather than putting a placeholder on the clipboard. */
export function CopyButton({
  text,
  what,
  icon,
  label,
}: {
  text: string | null | (() => string | null);
  what: string;
  icon?: React.ReactNode;
  /** With a label the button is a full action button reading `label`, and
   *  says "Copied" for a moment after — for the places an icon alone would
   *  be too quiet, like beside Re-analyze. */
  label?: string;
}) {
  // One tri-state rather than two booleans that must never both be true.
  const [status, setStatus] = useState<"idle" | "copied" | "failed">("idle");
  const title =
    text === null
      ? "Nothing to copy yet"
      : status === "copied"
        ? "Copied!"
        : status === "failed"
          ? "Couldn't copy — clipboard unavailable"
          : `Copy ${what}`;
  const flash = (next: "copied" | "failed") => {
    setStatus(next);
    setTimeout(() => setStatus("idle"), next === "copied" ? 1500 : 2500);
  };
  const copy = () => {
    const value = typeof text === "function" ? text() : text;
    if (value == null) return void flash("failed");
    void navigator.clipboard
      .writeText(value)
      .then(() => flash("copied"))
      .catch(() => flash("failed"));
  };
  if (label) {
    return (
      <button className="action-btn" disabled={text === null} {...tip(title)} onClick={copy}>
        {status === "copied" ? "Copied" : status === "failed" ? "Couldn't copy" : label}
      </button>
    );
  }
  return (
    <button className="icon-btn" disabled={text === null} {...tip(title)} onClick={copy}>
      {status === "copied" ? <IconCheck /> : (icon ?? <IconClipboard />)}
    </button>
  );
}
