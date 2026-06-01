import { expect, test } from "bun:test";
import { buildTree, flattenVisible } from "@/features/symbols/tree";

test("buildTree nests directories and places files, dirs sorted first", () => {
  const tree = buildTree([
    { path: "src/b.rs", symbol_count: 2 },
    { path: "src/a/c.rs", symbol_count: 1 },
    { path: "x.rs", symbol_count: 5 },
  ]);

  // Top level: dir "src" before file "x.rs".
  expect(tree.length).toBe(2);
  expect(tree[0]!.type).toBe("dir");
  expect(tree[0]!.name).toBe("src");
  expect(tree[1]!.type).toBe("file");
  expect(tree[1]!.name).toBe("x.rs");

  // Inside src: dir "a" before file "b.rs".
  const src = tree[0]!;
  if (src.type !== "dir") throw new Error("expected dir");
  expect(src.children[0]!.name).toBe("a");
  expect(src.children[0]!.type).toBe("dir");
  expect(src.children[1]!.name).toBe("b.rs");

  // Leaf file carries its full path and count.
  const b = src.children[1]!;
  if (b.type !== "file") throw new Error("expected file");
  expect(b.path).toBe("src/b.rs");
  expect(b.symbolCount).toBe(2);
});

test("flattenVisible expands only open dirs and carries depth", () => {
  const tree = buildTree([
    { path: "src/a/c.rs", symbol_count: 1 },
    { path: "src/b.rs", symbol_count: 2 },
  ]);

  // Nothing expanded: only the top-level dir row, at depth 0.
  const collapsed = flattenVisible(tree, new Set());
  expect(collapsed.map((r) => r.path)).toEqual(["src"]);
  expect(collapsed[0]!.type).toBe("dir");
  expect(collapsed[0]!.depth).toBe(0);

  // src open, nested dir still collapsed: src, src/a (dir), src/b.rs (file).
  const srcOpen = flattenVisible(tree, new Set(["src"]));
  expect(srcOpen.map((r) => r.path)).toEqual(["src", "src/a", "src/b.rs"]);

  // Both open: the nested file appears, at depth 2.
  const allOpen = flattenVisible(tree, new Set(["src", "src/a"]));
  expect(allOpen.map((r) => r.path)).toEqual(["src", "src/a", "src/a/c.rs", "src/b.rs"]);
  const leaf = allOpen.find((r) => r.path === "src/a/c.rs")!;
  expect(leaf.type).toBe("file");
  expect(leaf.depth).toBe(2);
});
