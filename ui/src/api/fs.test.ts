import { test, expect, afterEach, mock } from "bun:test";
import { listDir } from "@/api/fs";

afterEach(() => mock.restore());

test("listDir requests /api/fs/list with encoded path and show_hidden", async () => {
  let calledUrl = "";
  globalThis.fetch = mock(async (url: string) => {
    calledUrl = String(url);
    return new Response(JSON.stringify({ path: "/Users/x", parent: "/Users", entries: [] }), {
      status: 200,
    });
  }) as unknown as typeof fetch;

  const res = await listDir("/Users/x/My Repos", true);
  expect(calledUrl).toContain("/api/fs/list?");
  expect(calledUrl).toContain("path=%2FUsers%2Fx%2FMy+Repos");
  expect(calledUrl).toContain("show_hidden=true");
  expect(res.path).toBe("/Users/x");
});

test("listDir omits path param when none given", async () => {
  let calledUrl = "";
  globalThis.fetch = mock(async (url: string) => {
    calledUrl = String(url);
    return new Response(JSON.stringify({ path: "/home", parent: "/", entries: [] }), {
      status: 200,
    });
  }) as unknown as typeof fetch;

  await listDir();
  expect(calledUrl).toContain("/api/fs/list");
  expect(calledUrl).not.toContain("path=");
});
