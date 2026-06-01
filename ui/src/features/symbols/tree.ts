import type { IndexedFile } from "@/api/symbols";

export type TreeNode =
  | { type: "dir"; name: string; path: string; children: TreeNode[] }
  | { type: "file"; name: string; path: string; symbolCount: number };

type DirBuilder = { name: string; path: string; dirs: Map<string, DirBuilder>; files: TreeNode[] };

function emptyDir(name: string, path: string): DirBuilder {
  return { name, path, dirs: new Map(), files: [] };
}

function finalize(dir: DirBuilder): TreeNode[] {
  const dirNodes: TreeNode[] = Array.from(dir.dirs.values())
    .sort((a, b) => a.name.localeCompare(b.name))
    .map((d) => ({ type: "dir", name: d.name, path: d.path, children: finalize(d) }));
  const fileNodes = [...dir.files].sort((a, b) => a.name.localeCompare(b.name));
  return [...dirNodes, ...fileNodes];
}

export type FlatRow =
  | { type: "dir"; name: string; path: string; depth: number; open: boolean }
  | { type: "file"; name: string; path: string; depth: number; symbolCount: number };

/**
 * Flatten the tree into the rows that are currently visible (children of a
 * directory appear only when its path is in `expanded`), tagging each with its
 * depth. This is the windowing input for the virtualized FileTree.
 */
export function flattenVisible(
  nodes: TreeNode[],
  expanded: Set<string>,
  depth = 0,
): FlatRow[] {
  const out: FlatRow[] = [];
  for (const node of nodes) {
    if (node.type === "file") {
      out.push({ type: "file", name: node.name, path: node.path, depth, symbolCount: node.symbolCount });
    } else {
      const open = expanded.has(node.path);
      out.push({ type: "dir", name: node.name, path: node.path, depth, open });
      if (open) {
        out.push(...flattenVisible(node.children, expanded, depth + 1));
      }
    }
  }
  return out;
}

/** All directory paths in the tree (used to expand everything by default). */
export function allDirPaths(nodes: TreeNode[]): string[] {
  const out: string[] = [];
  for (const node of nodes) {
    if (node.type === "dir") {
      out.push(node.path);
      out.push(...allDirPaths(node.children));
    }
  }
  return out;
}

/** Build a nested directory tree from a flat file list. Dirs sort before files. */
export function buildTree(files: IndexedFile[]): TreeNode[] {
  const root = emptyDir("", "");
  for (const f of files) {
    const parts = f.path.split("/");
    let cursor = root;
    let acc = "";
    for (let i = 0; i < parts.length; i++) {
      const part = parts[i]!;
      acc = acc ? `${acc}/${part}` : part;
      if (i === parts.length - 1) {
        cursor.files.push({ type: "file", name: part, path: f.path, symbolCount: f.symbol_count });
      } else {
        let next = cursor.dirs.get(part);
        if (!next) {
          next = emptyDir(part, acc);
          cursor.dirs.set(part, next);
        }
        cursor = next;
      }
    }
  }
  return finalize(root);
}
