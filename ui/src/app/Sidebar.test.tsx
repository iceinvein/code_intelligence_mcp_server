import { afterEach, expect, mock, test } from "bun:test";
import { render, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { Sidebar } from "@/app/Sidebar";

afterEach(() => {
  mock.restore();
});

function renderSidebar(pendingCount: number) {
  globalThis.fetch = mock(async () =>
    new Response(
      JSON.stringify({
        pending: Array.from({ length: pendingCount }, (_v, i) => ({
          repo_path: `/r/${i}`,
          repo_id: `id${i}`,
          detected: "git_worktree",
          recommendation: "ask",
          detail: null,
          first_seen_unix_s: 1,
          last_seen_unix_s: 1,
          occurrences: 1,
        })),
        declined: [],
      }),
      { status: 200 },
    ),
  ) as unknown as typeof fetch;

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <Sidebar />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

test("shows a pending-count badge on the consent item", async () => {
  const { findByLabelText } = renderSidebar(2);
  const badge = await findByLabelText("2 pending");
  expect(badge.textContent).toBe("2");
});

test("hides the badge when nothing is pending", async () => {
  const { queryByLabelText } = renderSidebar(0);
  await waitFor(() => expect(queryByLabelText(/pending/)).toBeNull());
});
