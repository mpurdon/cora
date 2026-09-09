import { describe, expect, it } from "vitest";
import { parseDiff } from "./DiffView";

const NO_NEWLINE_MARKER = "\\ No newline at end of file";

describe("parseDiff", () => {
  it("skips the no-newline marker after a deleted line without shifting new-side numbering", () => {
    const raw = [
      "diff --git a/main.py b/main.py",
      "index 1111111..2222222 100644",
      "--- a/main.py",
      "+++ b/main.py",
      "@@ -1,4 +1,5 @@",
      " import os",
      " ",
      " def main():",
      "-    return 1",
      NO_NEWLINE_MARKER,
      "+    return 2",
      "+    # trailing",
      "",
    ].join("\n");

    const files = parseDiff(raw);
    expect(files).toHaveLength(1);
    const lines = files[0].lines;

    expect(lines.some((l) => l.text === NO_NEWLINE_MARKER)).toBe(false);

    const added = lines.filter((l) => l.kind === "add");
    expect(added).toHaveLength(2);
    expect(added[0]).toMatchObject({ text: "    return 2", newLine: 4 });
    expect(added[1]).toMatchObject({ text: "    # trailing", newLine: 5 });
  });

  it("skips the no-newline marker after an added/context line without shifting new-side numbering", () => {
    const raw = [
      "diff --git a/main.py b/main.py",
      "index 1111111..2222222 100644",
      "--- a/main.py",
      "+++ b/main.py",
      "@@ -1,3 +1,4 @@",
      " import os",
      " def main():",
      "-    return 1",
      "+    return 2",
      NO_NEWLINE_MARKER,
      "+    # trailing",
      "",
    ].join("\n");

    const files = parseDiff(raw);
    expect(files).toHaveLength(1);
    const lines = files[0].lines;

    expect(lines.some((l) => l.text === NO_NEWLINE_MARKER)).toBe(false);

    const added = lines.filter((l) => l.kind === "add");
    expect(added).toHaveLength(2);
    expect(added[0]).toMatchObject({ text: "    return 2", newLine: 3 });
    expect(added[1]).toMatchObject({ text: "    # trailing", newLine: 4 });
  });
});
