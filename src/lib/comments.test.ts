import { describe, expect, it } from "vitest";
import type { CodeFinding } from "../bindings/CodeFinding";
import type { PrConversation } from "../bindings/PrConversation";
import { findingId, findingMarker, isFindingCommented, viewerComments } from "./comments";

const ME = "mpurdon";

const finding: CodeFinding = {
  path: "libs/util-sst/src/lib/x.spec.ts",
  line: 90,
  severity: "high",
  kind: "defect",
  finding: "this test is vacuous",
  suggestion: "add a non-excluded function",
};

function convo(thread: { path: string; line: number; body: string }): PrConversation {
  return {
    comments: [],
    reviews: [],
    threads: [
      {
        id: "t",
        path: thread.path,
        line: thread.line,
        startLine: null,
        resolved: false,
        outdated: false,
        comments: [
          {
            id: "c",
            author: ME,
            isBot: false,
            body: thread.body,
            createdAt: "",
            url: "",
            reactions: [],
            viewerCanEdit: true,
            isReviewComment: true,
          },
        ],
      },
    ],
  };
}

describe("isFindingCommented", () => {
  it("matches a comment carrying the finding's marker, wherever it sits", () => {
    // The real case: the assistant anchored its comment at line 104, the
    // finding points at line 90, and position alone found nothing.
    const c = convo({ path: finding.path, line: 104, body: `noted\n\n${findingMarker(finding)}` });
    expect(isFindingCommented(finding, viewerComments(c, ME))).toBe(true);
  });

  it("still matches a comment sitting on the finding's own line", () => {
    const c = convo({ path: finding.path, line: 90, body: "noted" });
    expect(isFindingCommented(finding, viewerComments(c, ME))).toBe(true);
  });

  it("does not match an untagged comment elsewhere in the file", () => {
    const c = convo({ path: finding.path, line: 104, body: "unrelated thought" });
    expect(isFindingCommented(finding, viewerComments(c, ME))).toBe(false);
  });

  it("ignores a marker in someone else's comment", () => {
    const c = convo({ path: finding.path, line: 104, body: findingMarker(finding) });
    c.threads[0].comments[0].author = "someone-else";
    expect(isFindingCommented(finding, viewerComments(c, ME))).toBe(false);
  });

  it("keys the id on path and wording, not list position", () => {
    expect(findingId(finding)).toBe(findingId({ ...finding, line: 999, severity: "low" }));
    expect(findingId(finding)).not.toBe(findingId({ ...finding, finding: "something else" }));
    // The id Rust computes for the same key (chat::finding_id).
    expect(findingId({ ...finding, path: "a", finding: "b" })).toBe("294c7dd6");
  });
});
