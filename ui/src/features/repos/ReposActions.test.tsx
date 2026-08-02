import type React from "react";
import { test, expect, mock, afterEach } from "bun:test";
import { render, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ReposView } from "@/features/repos/ReposView";

afterEach(() => {
  mock.restore();
});

const REPO = {
  id: "abc",
  name: "demo-repo",
  path: "/repo/demo",
  data_dir: "/data/demo",
  created_at: "x",
  last_accessed: "x",
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
};

function renderWithClient(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

test("clicking reindex POSTs to the repo reindex route", async () => {
  const originalError = console.error;
  const consoleErrors: unknown[][] = [];
  console.error = mock((...args: unknown[]) => {
    consoleErrors.push(args);
    originalError(...args);
  }) as unknown as typeof console.error;

  try {
    const calls: Array<{ url: string; method: string }> = [];
    globalThis.fetch = mock(async (url: string, init?: RequestInit) => {
      calls.push({ url: String(url), method: init?.method ?? "GET" });
      if (String(url).endsWith("/reindex")) {
        return new Response(
          JSON.stringify({
            status: "started",
            job_id: "j",
            repo_id: "abc",
            repo_path: "/repo/demo",
          }),
          { status: 202 },
        );
      }
      return new Response(JSON.stringify({ count: 1, repos: [REPO] }), { status: 200 });
    }) as unknown as typeof fetch;

    const { findByText } = renderWithClient(<ReposView />);
    const reindexBtn = await findByText("reindex");
    fireEvent.click(reindexBtn);

    await waitFor(() => {
      const reindexCall = calls.find((c) => c.url.endsWith("/api/repos/abc/reindex"));
      expect(reindexCall).toBeDefined();
      expect(reindexCall!.method).toBe("POST");
    });
    expect(
      consoleErrors.filter((args) => String(args[0]).includes("not wrapped in act")),
    ).toHaveLength(0);
  } finally {
    console.error = originalError;
  }
});
