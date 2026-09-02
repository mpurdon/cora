import { useState } from "react";
import { CommentBody, useOverflows } from "./CommentsView";

/** How tall the description gets before it's clamped behind a fade. A PR
 *  template with a summary and a test plan fits; a pasted design doc doesn't. */
const DESCRIPTION_CLAMP_PX = 280;

/** The PR description, above the analysis on the Assessment tab. The author's
 *  stated intent is what you read to decide whether the (paid) analysis is
 *  worth running, so it shows before any analysis exists. Renders nothing for
 *  an empty body — no placeholder to scroll past. */
export function PrDescription({ body }: { body: string }) {
  const [open, setOpen] = useState(true);
  const [expanded, setExpanded] = useState(false);
  const [long, bodyRef] = useOverflows<HTMLDivElement>(DESCRIPTION_CLAMP_PX);
  if (!body.trim()) return null;
  return (
    <section className="pr-description">
      <button
        className="pr-description-head"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        <span className="eyebrow">Description</span>
        <span className="detail-toggle">{open ? "▾ hide" : "▸ show"}</span>
      </button>
      {/* Hidden rather than unmounted: the clamp hook observes this element,
          and a re-mount would leave it watching a detached one. */}
      <div hidden={!open}>
        <div
          className={long && !expanded ? "comment-clamped" : undefined}
          style={{ "--comment-clamp": `${DESCRIPTION_CLAMP_PX}px` } as React.CSSProperties}
        >
          {/* The measured element is the inner one: it is never clamped, so
              its height is the description's natural height in both states. */}
          <div ref={bodyRef}>
            <CommentBody body={body} />
          </div>
        </div>
        {long && (
          <button className="comment-expand" onClick={() => setExpanded((e) => !e)}>
            {expanded ? "show less" : "show more"}
          </button>
        )}
      </div>
    </section>
  );
}
