import { describe, expect, it } from "vitest";
import { parseDiff } from "./DiffView";

describe("parseDiff", () => {
  it("keeps `--- `/`+++ ` file headers before the first hunk", () => {
    const raw = [
      "diff --git a/schema.sql b/schema.sql",
      "index e69de29..9daeafb 100644",
      "--- a/schema.sql",
      "+++ b/schema.sql",
      "@@ -1,2 +1,2 @@",
      " CREATE TABLE foo (",
      " id INT",
    ].join("\n");

    const files = parseDiff(raw);
    expect(files).toHaveLength(1);
    expect(files[0].path).toBe("schema.sql");
    expect(files[0].oldPath).toBeUndefined();
    // The file-header `---`/`+++` lines are dropped, not rendered as content.
    expect(files[0].lines.map((l) => l.text)).not.toContain("a/schema.sql");
    expect(files[0].lines.map((l) => l.kind)).toEqual(["hunk", "ctx", "ctx"]);
  });

  it("treats `-- `/`++ ` content lines inside a hunk as ordinary del/add lines", () => {
    const raw = [
      "diff --git a/schema.sql b/schema.sql",
      "index e69de29..9daeafb 100644",
      "--- a/schema.sql",
      "+++ b/schema.sql",
      "@@ -1,2 +1,3 @@",
      " CREATE TABLE foo (",
      "--- legacy column, drop in v3",
      "+++ counter",
      " id INT",
    ].join("\n");

    const files = parseDiff(raw);
    expect(files).toHaveLength(1);
    const file = files[0];

    expect(file.deletions).toBe(1);
    expect(file.additions).toBe(1);

    const del = file.lines.find((l) => l.kind === "del");
    expect(del?.text).toBe("-- legacy column, drop in v3");

    const add = file.lines.find((l) => l.kind === "add");
    expect(add?.text).toBe("++ counter");
  });
});
