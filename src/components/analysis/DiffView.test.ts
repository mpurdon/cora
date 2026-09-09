import { describe, expect, it } from "vitest";
import { parseDiff } from "./DiffView";

describe("parseDiff", () => {
  it("treats a deleted comment line starting with '-- ' as content, not a file header", () => {
    const raw = [
      "diff --git a/schema.sql b/schema.sql",
      "index 1111111..2222222 100644",
      "--- a/schema.sql",
      "+++ b/schema.sql",
      "@@ -1,2 +1,1 @@",
      " CREATE TABLE t (",
      "--- legacy column, drop in v3",
      " );",
      "",
    ].join("\n");

    const files = parseDiff(raw);
    expect(files).toHaveLength(1);
    const file = files[0];

    expect(file.deletions).toBe(1);
    const del = file.lines.filter((l) => l.kind === "del");
    expect(del).toHaveLength(1);
    expect(del[0].text).toBe("-- legacy column, drop in v3");
  });

  it("treats an added line starting with '++ ' as content, not a file header", () => {
    const raw = [
      "diff --git a/counter.lua b/counter.lua",
      "index 1111111..2222222 100644",
      "--- a/counter.lua",
      "+++ b/counter.lua",
      "@@ -1,1 +1,2 @@",
      " local n = 0",
      "+++ counter",
      "",
    ].join("\n");

    const files = parseDiff(raw);
    expect(files).toHaveLength(1);
    const file = files[0];

    expect(file.additions).toBe(1);
    const add = file.lines.filter((l) => l.kind === "add");
    expect(add).toHaveLength(1);
    expect(add[0].text).toBe("++ counter");
  });

  it("still treats '--- '/'+++ ' file-name headers before the first hunk as headers", () => {
    const raw = [
      "diff --git a/a.txt b/a.txt",
      "index 1111111..2222222 100644",
      "--- a/a.txt",
      "+++ b/a.txt",
      "@@ -1,1 +1,1 @@",
      "-old",
      "+new",
      "",
    ].join("\n");

    const files = parseDiff(raw);
    expect(files).toHaveLength(1);
    const file = files[0];
    expect(file.path).toBe("a.txt");
    expect(file.lines.map((l) => l.kind)).toEqual(["hunk", "del", "add", "ctx"]);
  });
});
