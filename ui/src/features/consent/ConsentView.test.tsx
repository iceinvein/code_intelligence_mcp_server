import { afterEach, expect, mock, test } from "bun:test";
import { fireEvent, render, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ConsentView } from "@/features/consent/ConsentView";

afterEach(() => {
  mock.restore();
});

function renderView() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ConsentView />
    </QueryClientProvider>,
  );
}

test("renders pending + declined and posts the right decisions", async () => {
  const posts: Array<Record<string, unknown>> = [];
  globalThis.fetch = mock(async (url: string, init?: RequestInit) => {
    const u = String(url);
    if (u.endsWith("/api/consent") && init?.method === "POST") {
      posts.push(JSON.parse((init.body as string) ?? "{}"));
      return new Response(
        JSON.stringify({ ok: true, status: "declined", repo: "x", repo_id: "x" }),
        { status: 200 },
      );
    }
    return new Response(
      JSON.stringify({
        pending: [
          {
            repo_path: "/r/wt",
            repo_id: "p1",
            detected: "git_worktree",
            recommendation: "ask before indexing",
            detail: "git worktree of /r",
            first_seen_unix_s: 1,
            last_seen_unix_s: 1,
            occurrences: 1,
          },
        ],
        declined: [{ repo_path: "/r/old", repo_id: "d1", detected: "temp_dir" }],
      }),
      { status: 200 },
    );
  }) as unknown as typeof fetch;

  const { findByText, getByText } = renderView();

  expect(await findByText("/r/wt")).toBeDefined();
  expect(await findByText("/r/old")).toBeDefined();

  fireEvent.click(getByText("approve"));
  await waitFor(() => expect(posts.length).toBe(1));
  fireEvent.click(getByText("re-approve"));
  await waitFor(() => expect(posts.length).toBe(2));
  fireEvent.click(getByText("decline"));
  await waitFor(() => expect(posts.length).toBe(3));

  expect(posts).toContainEqual({ repo: "/r/wt", decision: "approve" });
  expect(posts).toContainEqual({ repo: "/r/old", decision: "approve" });
  expect(posts).toContainEqual({ repo: "/r/wt", decision: "decline" });
});

test("shows the empty state when nothing is pending or declined", async () => {
  globalThis.fetch = mock(async () =>
    new Response(JSON.stringify({ pending: [], declined: [] }), { status: 200 }),
  ) as unknown as typeof fetch;

  const { findByText } = renderView();
  expect(await findByText(/No repositories are awaiting a decision/)).toBeDefined();
});
