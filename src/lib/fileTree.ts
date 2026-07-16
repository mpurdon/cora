import type { DiffFile } from "../components/analysis/DiffView";

/** Directory node of the changed-files tree. Single-child directory chains
 *  are compressed into one row (`src/components/analysis`), IDE-style. */
export interface TreeDir {
  name: string;
  path: string;
  dirs: Map<string, TreeDir>;
  files: DiffFile[];
}

export function buildTree(files: DiffFile[]): TreeDir {
  const root: TreeDir = { name: "", path: "", dirs: new Map(), files: [] };
  for (const f of files) {
    const parts = f.path.split("/");
    let node = root;
    for (const part of parts.slice(0, -1)) {
      let child = node.dirs.get(part);
      if (!child) {
        child = {
          name: part,
          path: node.path ? `${node.path}/${part}` : part,
          dirs: new Map(),
          files: [],
        };
        node.dirs.set(part, child);
      }
      node = child;
    }
    node.files.push(f);
  }
  compressChains(root);
  return root;
}

function compressChains(node: TreeDir) {
  for (const [name, original] of [...node.dirs]) {
    let child = original;
    while (child.files.length === 0 && child.dirs.size === 1) {
      const [only] = child.dirs.values();
      child = { ...only, name: `${child.name}/${only.name}` };
    }
    node.dirs.delete(name);
    node.dirs.set(child.name, child);
    compressChains(child);
  }
}

export function countFiles(dir: TreeDir): number {
  let n = dir.files.length;
  for (const d of dir.dirs.values()) n += countFiles(d);
  return n;
}

export const fileName = (path: string) => path.split("/").pop() ?? path;

export const sortedDirs = (dir: TreeDir) =>
  [...dir.dirs.values()].sort((a, b) => a.name.localeCompare(b.name));

export const sortedFiles = (dir: TreeDir) =>
  [...dir.files].sort((a, b) => fileName(a.path).localeCompare(fileName(b.path)));

/** Paths in the exact order the rail tree renders them (dirs first,
 *  alphabetical) — the Diff tab lists files in this same order so the two
 *  panes always agree. */
export function treeFileOrder(files: DiffFile[]): string[] {
  const order: string[] = [];
  const walk = (dir: TreeDir) => {
    for (const d of sortedDirs(dir)) walk(d);
    for (const f of sortedFiles(dir)) order.push(f.path);
  };
  walk(buildTree(files));
  return order;
}
