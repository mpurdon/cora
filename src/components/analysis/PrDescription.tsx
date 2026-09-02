import { CommentBody } from "./CommentsView";

/** The Description tab: the author's stated intent, which is what you read to
 *  decide whether the (paid) analysis is worth running. On a tab of its own
 *  there's nothing to fold or clamp — the whole body scrolls. */
export function PrDescription({ body }: { body: string }) {
  if (!body.trim()) {
    return (
      <div className="pr-description">
        <p className="pr-description-empty">This pull request has no description.</p>
      </div>
    );
  }
  return (
    <div className="pr-description">
      <CommentBody body={body} />
    </div>
  );
}
