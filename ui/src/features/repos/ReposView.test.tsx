import type React from "react";
import { test, expect, mock, afterEach } from "bun:test";
import { render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ReposView } from "@/features/repos/ReposView";

afterEach(() => {
  mock.restore();
});

function renderWithClient(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

test("ReposView renders repo names from the API", async () => {
  const payload = {
    count: 1,
    repos: [
      {
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
      },
    ],
  };
  globalThis.fetch = mock(async () => new Response(JSON.stringify(payload), { status: 200 })) as unknown as typeof fetch;

  const { findByText } = renderWithClient(<ReposView />);
  expect(await findByText("demo-repo")).toBeDefined();
  expect(await findByText("/repo/demo")).toBeDefined();
});

test("ReposView flags a repo whose checkout no longer exists on disk", async () => {
  const payload = {
    count: 1,
    repos: [
      {
        id: "abc",
        name: "gone-repo",
        path: "/repo/gone",
        data_dir: "/data/gone",
        created_at: "x",
        last_accessed: "x",
        path_exists: false,
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

  const { findByText } = renderWithClient(<ReposView />);
  expect(await findByText("gone-repo")).toBeDefined();
  expect(await findByText("stale")).toBeDefined();
});

test("ReposView counts down to automatic deletion when a deadline is set", async () => {
  const inFiveDays = new Date(Date.now() + 5 * 86_400_000).toISOString();
  const payload = {
    count: 1,
    repos: [
      {
        id: "abc",
        name: "doomed-repo",
        path: "/repo/doomed",
        data_dir: "/data/doomed",
        created_at: "x",
        last_accessed: "x",
        path_exists: false,
        seeded_from: null,
        missing_since: new Date(Date.now() - 2 * 86_400_000).toISOString(),
        auto_delete_at: inFiveDays,
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

  const { findByText } = renderWithClient(<ReposView />);
  expect(await findByText("stale · deletes in 5d")).toBeDefined();
});
