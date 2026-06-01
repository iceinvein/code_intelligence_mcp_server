import { afterEach, expect, mock, test } from "bun:test";
import { fetchConsent, resolveConsent } from "@/api/consent";

afterEach(() => {
  mock.restore();
});

test("fetchConsent returns the pending and declined lists", async () => {
  globalThis.fetch = mock(async () =>
    new Response(
      JSON.stringify({
        pending: [
          {
            repo_path: "/r/a",
            repo_id: "id1",
            detected: "git_worktree",
            recommendation: "ask",
            detail: "git worktree of /r",
            first_seen_unix_s: 1,
            last_seen_unix_s: 2,
            occurrences: 3,
          },
        ],
        declined: [{ repo_path: "/r/b", repo_id: "id2", detected: "temp_dir" }],
      }),
      { status: 200 },
    ),
  ) as unknown as typeof fetch;

  const data = await fetchConsent();
  expect(data.pending[0]!.repo_id).toBe("id1");
  expect(data.pending[0]!.occurrences).toBe(3);
  expect(data.declined[0]!.detected).toBe("temp_dir");
});

test("resolveConsent posts repo + decision to /api/consent", async () => {
  const calls: Array<{ url: string; body: string | null }> = [];
  globalThis.fetch = mock(async (url: string, init?: RequestInit) => {
    calls.push({ url: String(url), body: (init?.body as string) ?? null });
    return new Response(
      JSON.stringify({ ok: true, status: "declined", repo: "/r/a", repo_id: "id1" }),
      { status: 200 },
    );
  }) as unknown as typeof fetch;

  const res = await resolveConsent("/r/a", "decline");
  expect(res.status).toBe("declined");
  expect(calls[0]!.url).toBe("/api/consent");
  expect(JSON.parse(calls[0]!.body!)).toEqual({ repo: "/r/a", decision: "decline" });
});
