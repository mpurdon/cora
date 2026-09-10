import { describe, expect, it } from "vitest";
import { parseDiff } from "./DiffView";

describe("parseDiff", () => {
  it("keeps a deleted '-- comment' line as a del row, not a file header", () => {
    const raw = [
      "diff --git a/schema.sql b/schema.sql",
      "index 1111111..2222222 100644",
      "--- a/schema.sql",
      "+++ b/schema.sql",
      "@@ -1,2 +1,1 @@",
      "-- legacy column, drop in v3",
      " CREATE TABLE foo (id INT);",
      "",
    ].join("\n");

    const [file] = parseDiff(raw);
    expect(file.deletions).toBe(1);
    expect(file.lines).toContainEqual(
      expect.objectContaining({ kind: "del", text: "- legacy column, drop in v3" }),
    );
  });

  it("keeps an added '++ counter' line as an add row, not a file header", () => {
    const raw = [
      "diff --git a/main.c b/main.c",
      "index 1111111..2222222 100644",
      "--- a/main.c",
      "+++ b/main.c",
      "@@ -1,1 +1,2 @@",
      " int x = 0;",
      "++ counter",
      "",
    ].join("\n");

    const [file] = parseDiff(raw);
    expect(file.additions).toBe(1);
    expect(file.lines).toContainEqual(
      expect.objectContaining({ kind: "add", text: "+ counter" }),
    );
  });

  it("still treats the standard '---'/'+++' file-name lines before the hunk as headers", () => {
    const raw = [
      "diff --git a/foo.txt b/foo.txt",
      "index 1111111..2222222 100644",
      "--- a/foo.txt",
      "+++ b/foo.txt",
      "@@ -1,1 +1,1 @@",
      "-old",
      "+new",
    ].join("\n");

    const [file] = parseDiff(raw);
    expect(file.path).toBe("foo.txt");
    expect(file.lines.map((l) => l.kind)).toEqual(["hunk", "del", "add"]);
  });
});
