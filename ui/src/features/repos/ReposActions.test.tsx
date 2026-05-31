import type React from "react";
import { test, expect, mock, afterEach } from "bun:test";
import { render, fireEvent } from "@testing-library/react";
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
  const calls: Array<{ url: string; method: string }> = [];
  globalThis.fetch = mock(async (url: string, init?: RequestInit) => {
    calls.push({ url: String(url), method: init?.method ?? "GET" });
    if (String(url).endsWith("/reindex")) {
      return new Response(JSON.stringify({ status: "started", job_id: "j", repo_id: "abc", repo_path: "/repo/demo" }), { status: 202 });
    }
    return new Response(JSON.stringify({ count: 1, repos: [REPO] }), { status: 200 });
  }) as unknown as typeof fetch;

  const { findByText } = renderWithClient(<ReposView />);
  const reindexBtn = await findByText("reindex");
  fireEvent.click(reindexBtn);

  await Bun.sleep(20);
  const reindexCall = calls.find((c) => c.url.endsWith("/api/repos/abc/reindex"));
  expect(reindexCall).toBeDefined();
  expect(reindexCall!.method).toBe("POST");
});
