import type React from "react";
import { test, expect, mock, afterEach } from "bun:test";
import { render, fireEvent, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ReposView } from "@/features/repos/ReposView";

afterEach(() => {
  mock.restore();
});

function renderWithClient(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

// happy-dom + React 18 event delegation does not trigger React synthetic onChange
// from fireEvent.change (the event's `isTrusted` is undefined and happy-dom's
// event dispatch path bypasses React's root-container listener). Calling the
// __reactProps onChange directly is the idiomatic workaround in this environment.
function reactChange(el: HTMLInputElement, value: string) {
  const propsKey = Object.keys(el).find((k) => k.startsWith("__reactProps"));
  if (!propsKey) throw new Error("No __reactProps found on element");
  const props = (el as unknown as Record<string, Record<string, (e: Partial<React.ChangeEvent<HTMLInputElement>>) => void>>)[propsKey];
  props.onChange?.({ target: el, currentTarget: el, nativeEvent: new Event("change") } as React.ChangeEvent<HTMLInputElement>);
  // Also set the DOM value so subsequent reads return the right thing
  fireEvent.change(el, { target: { value } });
}

test("submitting the add-repo form POSTs the path to /api/repos", async () => {
  const calls: Array<{ url: string; method: string; body: string | null }> = [];
  globalThis.fetch = mock(async (url: string, init?: RequestInit) => {
    calls.push({ url: String(url), method: init?.method ?? "GET", body: (init?.body as string) ?? null });
    if (init?.method === "POST") {
      return new Response(
        JSON.stringify({ id: "n", name: "new", path: "/repo/new", data_dir: "/d", created_at: "x", last_accessed: "x" }),
        { status: 201 },
      );
    }
    return new Response(JSON.stringify({ count: 0, repos: [] }), { status: 200 });
  }) as unknown as typeof fetch;

  const { findByLabelText, findByText } = renderWithClient(<ReposView />);
  const input = (await findByLabelText("repository path to add")) as HTMLInputElement;

  // Set input value via the native DOM property so the button sees a non-empty path,
  // then trigger React's onChange handler directly (workaround for happy-dom + React 18).
  await act(async () => {
    const ownSetter = Object.getOwnPropertyDescriptor(input, "value")?.set;
    const protoSetter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(input), "value")?.set;
    (ownSetter ?? protoSetter)?.call(input, "/repo/new");
    reactChange(input, "/repo/new");
  });

  const btn = await findByText("add repo");
  await act(async () => {
    fireEvent.click(btn);
  });

  await new Promise((r) => setTimeout(r, 20));
  const post = calls.find((c) => c.method === "POST" && c.url.endsWith("/api/repos"));
  expect(post).toBeDefined();
  expect(JSON.parse(post!.body!)).toEqual({ path: "/repo/new" });
});
