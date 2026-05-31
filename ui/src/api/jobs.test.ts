import { test, expect, mock, afterEach } from "bun:test";
import { fetchJobs } from "@/api/jobs";
import { reindexRepo } from "@/api/repos";

afterEach(() => {
  mock.restore();
});

test("fetchJobs parses the /api/jobs envelope", async () => {
  const payload = {
    count: 1,
    running: 1,
    jobs: [
      {
        id: "reindex-abc-123",
        kind: "manual_reindex",
        repo_id: "abc",
        repo_path: "/repo/demo",
        status: "running",
        started_at_unix_s: 100,
        finished_at_unix_s: null,
        duration_ms: null,
        stats: null,
        error: null,
        coalesced_count: 0,
      },
    ],
  };
  globalThis.fetch = mock(async () => new Response(JSON.stringify(payload), { status: 200 })) as unknown as typeof fetch;

  const result = await fetchJobs();
  expect(result.running).toBe(1);
  expect(result.jobs[0]!.kind).toBe("manual_reindex");
  expect(result.jobs[0]!.status).toBe("running");
});

test("reindexRepo POSTs to the reindex route and parses the started envelope", async () => {
  const calls: Array<{ url: string; method: string }> = [];
  globalThis.fetch = mock(async (url: string, init?: RequestInit) => {
    calls.push({ url: String(url), method: init?.method ?? "GET" });
    return new Response(
      JSON.stringify({ status: "started", job_id: "j1", repo_id: "abc", repo_path: "/repo/demo" }),
      { status: 202 },
    );
  }) as unknown as typeof fetch;

  const result = await reindexRepo("abc");
  expect(result.status).toBe("started");
  expect(calls[0]!.method).toBe("POST");
  expect(calls[0]!.url).toBe("/api/repos/abc/reindex");
});
