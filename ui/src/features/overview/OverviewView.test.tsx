import type React from "react";
import { test, expect, mock, afterEach } from "bun:test";
import { render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { OverviewView } from "@/features/overview/OverviewView";

afterEach(() => {
  mock.restore();
});

function renderView(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>{ui}</MemoryRouter>
    </QueryClientProvider>,
  );
}

function jsonFor(url: string): Response {
  const u = String(url);
  if (u.endsWith("/api/status")) {
    return new Response(
      JSON.stringify({
        version: "4.3.0",
        started_at_unix_s: 1,
        uptime_s: 3720,
        registered_repos: 1,
        active_sessions: 1,
        connected_sessions: 1,
        bound_sessions: 1,
      }),
      { status: 200 },
    );
  }
  if (u.endsWith("/api/repos")) {
    return new Response(
      JSON.stringify({
        count: 1,
        repos: [
          {
            id: "r1",
            name: "code-intelligence-mcp",
            path: "/Users/x/code-intelligence",
            data_dir: "/d",
            created_at: "1",
            last_accessed: "1",
            path_exists: true,
            seeded_from: null,
            activity: {
              running: false,
              current: null,
              last_finished: null,
              latest_index_run: null,
              latest_search_run: null,
              last_updated_unix_s: 10,
            },
          },
        ],
      }),
      { status: 200 },
    );
  }
  if (u.endsWith("/api/jobs")) {
    return new Response(
      JSON.stringify({
        count: 1,
        running: 0,
        jobs: [
          {
            id: "j1",
            kind: "manual_reindex",
            repo_id: "r1",
            repo_path: "/Users/x/code-intelligence",
            status: "succeeded",
            started_at_unix_s: 1,
            finished_at_unix_s: 2,
            duration_ms: 1500,
            stats: null,
            error: null,
            coalesced_count: 0,
          },
        ],
      }),
      { status: 200 },
    );
  }
  // /api/sessions
  return new Response(
    JSON.stringify({ count: 1, bound_count: 1, connected_count: 2, sessions: [] }),
    { status: 200 },
  );
}

test("overview shows daemon vitals, a repository, and a recent job", async () => {
  globalThis.fetch = mock(async (url: string) => jsonFor(url)) as unknown as typeof fetch;

  const { findByText, findAllByText } = renderView(<OverviewView />);

  expect(await findByText("daemon running")).toBeDefined();
  expect(await findByText("code-intelligence-mcp")).toBeDefined();
  expect(await findByText("manual reindex")).toBeDefined();
  // version + uptime render in the daemon line
  expect((await findAllByText(/v4\.3\.0/)).length).toBeGreaterThan(0);
});

test("overview surfaces an unreachable daemon when status fails", async () => {
  globalThis.fetch = mock(async (url: string) => {
    if (String(url).endsWith("/api/status")) return new Response("nope", { status: 500 });
    return jsonFor(url);
  }) as unknown as typeof fetch;

  const { findByText } = renderView(<OverviewView />);
  expect(await findByText("daemon unreachable")).toBeDefined();
});
