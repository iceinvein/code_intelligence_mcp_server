import { test, expect, mock, afterEach } from "bun:test";
import { fetchRepoDetail, fetchRepos } from "@/api/repos";
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
        path_exists: true,
        seeded_from: null,
        missing_since: null,
        auto_delete_at: null,
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

test("fetchRepoDetail preserves external index stats", async () => {
  const payload = {
    id: "abc123",
    name: "demo",
    path: "/repo/demo",
    data_dir: "/data/demo",
    created_at: "2026-05-31T00:00:00Z",
    last_accessed: "2026-05-31T00:00:00Z",
    stats: {
      symbols: 10,
      edges: 20,
      descriptions: 3,
      undescribed_symbols: 7,
      last_updated_unix_s: 123,
      latest_index_run: null,
      latest_search_run: null,
      external_indexes: {
        index_count: 1,
        symbol_count: 4,
        reference_count: 5,
        mapped_symbol_count: 3,
      },
      external_producers: [
        {
          id: "rust",
          language: "rust",
          tier: "first_class",
          executable: "/bin/code-intelligence-external-rust",
          availability: "bundled",
        },
      ],
    },
  };
  globalThis.fetch = mock(async () => new Response(JSON.stringify(payload), { status: 200 })) as unknown as typeof fetch;

  const result = await fetchRepoDetail("abc123");

  expect(result.stats?.external_indexes?.index_count).toBe(1);
  expect(result.stats?.external_indexes?.reference_count).toBe(5);
  expect(result.stats?.external_producers?.[0]?.availability).toBe("bundled");
});
