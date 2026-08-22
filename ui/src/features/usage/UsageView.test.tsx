import type React from "react";
import { test, expect, mock, afterEach } from "bun:test";
import { render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { UsageView } from "@/features/usage/UsageView";

afterEach(() => {
  mock.restore();
});

function renderWithClient(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

const NOW = 1_760_000_000;

test("UsageView renders totals, a repo row, and a recent search", async () => {
  globalThis.fetch = mock(async (url: string) => {
    if (String(url).endsWith("/api/usage")) {
      return new Response(
        JSON.stringify({
          generated_at_unix_s: NOW,
          window_days: 14,
          totals: { searches: 42, cache_hits: 13 },
          repos: [
            {
              id: "r1",
              name: "my-repo",
              path: "/repo/x",
              search_total: 42,
              cache_hit_count: 13,
              avg_duration_ms: 210,
              last_search_at_unix_s: NOW - 60,
              index_run_count: 3,
              last_index_at_unix_s: NOW - 86_400,
              daily: [
                { day: new Date((NOW - 86_400) * 1000).toISOString().slice(0, 10), searches: 7 },
                { day: new Date(NOW * 1000).toISOString().slice(0, 10), searches: 12 },
              ],
            },
          ],
          recent_runs: [
            {
              repo_id: "r1",
              repo_name: "my-repo",
              started_at_unix_s: NOW - 30,
              duration_ms: 187,
              query_text: "how does auth work?",
              query_limit: 5,
              exported_only: false,
              result_count: 5,
              search_path: "single",
              cache_status: "hit",
            },
            {
              repo_id: "r1",
              repo_name: "my-repo",
              started_at_unix_s: NOW - 90,
              duration_ms: 402,
              query_text: null,
              query_limit: 5,
              exported_only: false,
              result_count: 4,
              search_path: "single",
              cache_status: "miss",
            },
          ],
        }),
        { status: 200 },
      );
    }
    return new Response(JSON.stringify({ error: "not found" }), { status: 404 });
  }) as unknown as typeof fetch;

  const { findByText, findAllByText } = renderWithClient(<UsageView />);
  expect(await findByText("how does auth work?")).toBeDefined();
  expect(await findByText("(query not stored)")).toBeDefined();
  expect(await findByText("31%")).toBeDefined();
  // Repo name appears once in the repos sheet and once per recent run.
  expect((await findAllByText("my-repo")).length).toBeGreaterThanOrEqual(3);
});
