import type React from "react";
import { test, expect, afterEach, mock } from "bun:test";
import { render, fireEvent, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ReposView } from "@/features/repos/ReposView";

afterEach(() => mock.restore());

function renderWithClient(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

test("Add repository opens the folder picker and shows the browser", async () => {
  globalThis.fetch = mock(async (url: string) => {
    if (String(url).includes("/api/fs/list")) {
      return new Response(
        JSON.stringify({
          path: "/home",
          parent: "/",
          entries: [{ name: "proj", path: "/home/proj", has_git: true, hidden: false }],
        }),
        { status: 200 },
      );
    }
    return new Response(JSON.stringify({ count: 0, repos: [] }), { status: 200 });
  }) as unknown as typeof fetch;

  const { findByText } = renderWithClient(<ReposView />);
  const openBtn = await findByText("add repository");
  await act(async () => {
    fireEvent.click(openBtn);
  });
  expect(await findByText("Select a repository folder")).toBeDefined();
  expect(await findByText("proj")).toBeDefined();
});
