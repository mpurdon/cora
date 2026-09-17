import { describe, expect, it } from "vitest";
import type { PrConversation } from "../bindings/PrConversation";
import type { PrReviews } from "../bindings/PrReviews";
import type { ReviewThread } from "../bindings/ReviewThread";
import type { TrackedPr } from "../bindings/TrackedPr";
import { teamsSeed } from "./teams";

const pr = {
  repo: "acme/widgets",
  number: 42,
  url: "https://github.com/acme/widgets/pull/42",
} as TrackedPr;

const me = "mpurdon";

function thread(path: string, body: string, resolved = false): ReviewThread {
  return {
    id: path,
    path,
    line: 1,
    startLine: null,
    resolved,
    outdated: false,
    comments: [
      {
        id: "c",
        author: me,
        isBot: false,
        body,
        createdAt: "",
        url: "",
        reactions: [],
        viewerCanEdit: false,
        isReviewComment: true,
      },
    ],
  };
}

function reviews(state?: string): PrReviews {
  return {
    requested: [],
    reviews: state ? [{ author: me, state, submittedAt: null }] : [],
    viewerLogin: me,
    lastCommitAt: null,
    openThreads: 0,
    myOpenThreads: 0,
    myDismissal: null,
  };
}

function convo(threads: ReviewThread[]): PrConversation {
  return { comments: [], threads, reviews: [] };
}

describe("teamsSeed", () => {
  it("says approved, in the approve composer's words, and ends with the link", () => {
    const text = teamsSeed(pr, reviews("APPROVED"), convo([thread("a.py", "fix this", true)]), me);
    expect(text).toBe(
      "Approved widgets#42 — my 1 comment on a.py is addressed.\n" + pr.url,
    );
  });

  it("counts what the author still has to look at after changes requested", () => {
    const c = convo([thread("a.py", "fix this"), thread("b.py", "note: optional")]);
    expect(teamsSeed(pr, reviews("CHANGES_REQUESTED"), c, me)).toContain(
      "Requested changes on widgets#42 — 1 comment to look at",
    );
  });

  it("reads as a comment drop when there is no verdict", () => {
    const c = convo([thread("a.py", "praise: nice"), thread("b.py", "fyi: see docs")]);
    expect(teamsSeed(pr, reviews(), c, me)).toBe(
      "Left 2 comments on widgets#42 — nothing blocking, back to you.\n" + pr.url,
    );
    expect(teamsSeed(pr, reviews(), convo([thread("a.py", "fix")]), me)).toContain(
      "Left 1 comment on widgets#42, back to you.",
    );
  });

  it("falls back to a bare mention with nothing to report", () => {
    expect(teamsSeed(pr, null, null, me)).toBe("Had a look at widgets#42.\n" + pr.url);
  });
});
