import { test, expect, mock, afterEach } from "bun:test";
import { render, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { SearchView } from "@/features/search/SearchView";

afterEach(() => {
  mock.restore();
});

function renderAt(url: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[url]}>
        <SearchView />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

test("SearchView renders search hits for the URL query", async () => {
  globalThis.fetch = mock(async (url: string) => {
    if (String(url).endsWith("/api/repos")) {
      return new Response(JSON.stringify({ count: 1, repos: [{ id: "abc", name: "demo", path: "/repo/demo", data_dir: "/d", created_at: "x", last_accessed: "x", activity: { running: false, current: null, last_finished: null, latest_index_run: null, latest_search_run: null, last_updated_unix_s: null } }] }), { status: 200 });
    }
    return new Response(
      JSON.stringify({ ok: true, command: "search", repo: { path: "/repo/demo", id: "abc" }, index: { version_unix_s: 1, fresh: true }, warnings: [], result: { query: "foo", limit: 25, hits: [{ id: "s1", name: "resolveState", kind: "fn", file_path: "src/x.rs", score: 0.91 }], hits_budget: { returned_count: 1, total_count: 1, truncated: false } } }),
      { status: 200 },
    );
  }) as unknown as typeof fetch;

  const { findByText } = renderAt("/search?repo=abc&q=foo");
  expect(await findByText("resolveState")).toBeDefined();
  expect(await findByText(/src\/x\.rs/)).toBeDefined();
});

test("SearchView auto-selects the repo when exactly one exists and none is in the URL", async () => {
  globalThis.fetch = mock(async (url: string) => {
    if (String(url).endsWith("/api/repos")) {
      return new Response(JSON.stringify({ count: 1, repos: [{ id: "abc", name: "demo", path: "/repo/demo", data_dir: "/d", created_at: "x", last_accessed: "x", activity: { running: false, current: null, last_finished: null, latest_index_run: null, latest_search_run: null, last_updated_unix_s: null } }] }), { status: 200 });
    }
    return new Response(JSON.stringify({ ok: true, command: "search", repo: { path: "/repo/demo", id: "abc" }, index: { version_unix_s: 1, fresh: true }, warnings: [], result: { query: "", limit: 25, hits: [], hits_budget: { returned_count: 0, total_count: 0, truncated: false } } }), { status: 200 });
  }) as unknown as typeof fetch;

  // No repo in the URL: the lone repo should be auto-selected into the <select>.
  const { getByLabelText } = renderAt("/search");
  await waitFor(() => {
    expect((getByLabelText("repository to search") as HTMLSelectElement).value).toBe("abc");
  });
});
