import type React from "react";
import { test, expect, afterEach, mock } from "bun:test";
import { render, fireEvent, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { FolderPickerDialog } from "@/features/repos/FolderPickerDialog";

afterEach(() => mock.restore());

function renderWithClient(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

function listingResponse(path: string, parent: string | null, names: string[]) {
  return new Response(
    JSON.stringify({
      path,
      parent,
      entries: names.map((name) => ({
        name,
        path: `${path}/${name}`,
        has_git: false,
        hidden: false,
      })),
    }),
    { status: 200 },
  );
}

test("renders entries and descends on click", async () => {
  const urls: string[] = [];
  globalThis.fetch = mock(async (url: string) => {
    urls.push(String(url));
    if (String(url).includes("path=")) return listingResponse("/home/proj", "/home", ["inner"]);
    return listingResponse("/home", "/", ["proj"]);
  }) as unknown as typeof fetch;

  const { findByText } = renderWithClient(<FolderPickerDialog open onOpenChange={() => {}} />);
  const row = await findByText("proj");
  await act(async () => {
    fireEvent.click(row);
  });
  expect(await findByText("inner")).toBeDefined();
  expect(urls.some((u) => u.includes("path=%2Fhome%2Fproj"))).toBe(true);
});

test("Add this folder POSTs the current path and closes", async () => {
  let posted: { url: string; body: string } | null = null;
  globalThis.fetch = mock(async (url: string, init?: RequestInit) => {
    if (init?.method === "POST") {
      posted = { url: String(url), body: String(init?.body) };
      return new Response(
        JSON.stringify({
          id: "n",
          name: "home",
          path: "/home",
          data_dir: "/d",
          created_at: "x",
          last_accessed: "x",
        }),
        { status: 201 },
      );
    }
    return listingResponse("/home", "/", ["proj"]);
  }) as unknown as typeof fetch;

  let openState = true;
  const onOpenChange = mock((v: boolean) => {
    openState = v;
  });
  const { findByText } = renderWithClient(<FolderPickerDialog open onOpenChange={onOpenChange} />);
  const addBtn = await findByText("Add this folder");
  await act(async () => {
    fireEvent.click(addBtn);
  });
  await new Promise((r) => setTimeout(r, 20));
  expect(posted).not.toBeNull();
  expect(JSON.parse(posted!.body)).toEqual({ path: "/home" });
  expect(onOpenChange).toHaveBeenCalledWith(false);
  expect(openState).toBe(false);
});
