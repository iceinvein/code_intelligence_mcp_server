import { test, expect } from "bun:test";
import { describeError } from "@/lib/errors";

test("maps the daemon repo-state error to indexing guidance", () => {
  const msg = describeError(
    new Error("failed to load repo: Failed to initialize repository state"),
    "search failed",
  );
  expect(msg).toContain("isn't indexed yet");
  expect(msg).toContain("reindex");
  // no stacked "failed:" prefixes leaking through
  expect(msg.toLowerCase()).not.toContain("failed:");
});

test("maps a missing-repo error to a selection hint", () => {
  expect(describeError(new Error("no repo bound"), "graph failed")).toContain(
    "no repository is selected",
  );
});

test("maps a network failure to a daemon-unreachable hint", () => {
  expect(describeError(new TypeError("Failed to fetch"), "search failed")).toContain(
    "can't reach the daemon",
  );
});

test("falls back to one clean lead plus the raw detail", () => {
  expect(describeError(new Error("boom"), "search failed")).toBe("search failed: boom");
});

test("uses the bare fallback when there is no message", () => {
  expect(describeError(undefined, "couldn't load settings")).toBe("couldn't load settings");
});
