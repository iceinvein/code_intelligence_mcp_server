import type React from "react";
import { test, expect, mock, afterEach } from "bun:test";
import { render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ActivityView } from "@/features/activity/ActivityView";

afterEach(() => {
  mock.restore();
});

function renderWithClient(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

test("ActivityView renders a job and a session from the API", async () => {
  globalThis.fetch = mock(async (url: string) => {
    if (String(url).endsWith("/api/jobs")) {
      return new Response(
        JSON.stringify({
          count: 1,
          running: 1,
          jobs: [{ id: "j", kind: "manual_reindex", repo_id: "a", repo_path: "/repo/x", status: "running", started_at_unix_s: 1, finished_at_unix_s: null, duration_ms: null, stats: null, error: null, coalesced_count: 0 }],
        }),
        { status: 200 },
      );
    }
    return new Response(
      JSON.stringify({
        count: 1,
        bound_count: 1,
        connected_count: 1,
        sessions: [{ session_id: "s1", repo: "/repo/x", bound: true, initialized_at_unix_s: 1, last_seen_secs_ago: 4, bind_skipped_reason: null }],
      }),
      { status: 200 },
    );
  }) as unknown as typeof fetch;

  const { findByText, findAllByText } = renderWithClient(<ActivityView />);
  expect(await findByText("manual_reindex")).toBeDefined();
  expect(await findByText("bound")).toBeDefined();
  expect((await findAllByText("/repo/x")).length).toBeGreaterThan(0);
});
