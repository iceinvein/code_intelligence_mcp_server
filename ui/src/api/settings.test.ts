import { afterEach, expect, mock, test } from "bun:test";
import { fetchSettings, putSettings } from "@/api/settings";

afterEach(() => {
  mock.restore();
});

test("fetchSettings returns the field catalog", async () => {
  globalThis.fetch = mock(async () =>
    new Response(
      JSON.stringify({
        fields: [
          {
            key: "hybrid_alpha",
            group: "Retrieval",
            type: "number",
            value: 0.7,
            default: 0.7,
            range: { min: 0, max: 1 },
            needs_restart: true,
            needs_reindex: false,
            editable: true,
            description: "vector vs keyword",
          },
        ],
      }),
      { status: 200 },
    ),
  ) as unknown as typeof fetch;

  const data = await fetchSettings();
  expect(data.fields[0]!.key).toBe("hybrid_alpha");
  expect(data.fields[0]!.range?.max).toBe(1);
});

test("putSettings PUTs a changes object to /api/settings", async () => {
  const calls: Array<{ url: string; method?: string; body: string | null }> = [];
  globalThis.fetch = mock(async (url: string, init?: RequestInit) => {
    calls.push({
      url: String(url),
      method: init?.method,
      body: (init?.body as string) ?? null,
    });
    return new Response(JSON.stringify({ ok: true, needs_restart: true, needs_reindex: false }), {
      status: 200,
    });
  }) as unknown as typeof fetch;

  const res = await putSettings({ hybrid_alpha: 0.8 });
  expect(res.ok).toBe(true);
  expect(calls[0]!.url).toBe("/api/settings");
  expect(calls[0]!.method).toBe("PUT");
  expect(JSON.parse(calls[0]!.body!)).toEqual({ changes: { hybrid_alpha: 0.8 } });
});
