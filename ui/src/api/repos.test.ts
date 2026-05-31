import { test, expect, mock, afterEach } from "bun:test";
import { fetchRepos } from "@/api/repos";
import { ApiError } from "@/api/client";

afterEach(() => {
  mock.restore();
});

test("fetchRepos parses the /api/repos envelope", async () => {
  const payload = {
    count: 1,
    repos: [
      {
        id: "abc123",
        name: "demo",
        path: "/repo/demo",
        data_dir: "/data/demo",
        created_at: "2026-05-31T00:00:00Z",
        last_accessed: "2026-05-31T00:00:00Z",
        activity: {
          running: false,
          current: null,
          last_finished: null,
          latest_index_run: null,
          latest_search_run: null,
          last_updated_unix_s: null,
        },
      },
    ],
  };
  globalThis.fetch = mock(async () => new Response(JSON.stringify(payload), { status: 200 })) as unknown as typeof fetch;

  const result = await fetchRepos();
  expect(result.count).toBe(1);
  expect(result.repos[0]!.id).toBe("abc123");
  expect(result.repos[0]!.activity.running).toBe(false);
});

test("apiGet surfaces the server error message as ApiError", async () => {
  globalThis.fetch = mock(
    async () => new Response(JSON.stringify({ error: "boom" }), { status: 500 }),
  ) as unknown as typeof fetch;

  await expect(fetchRepos()).rejects.toBeInstanceOf(ApiError);
});
