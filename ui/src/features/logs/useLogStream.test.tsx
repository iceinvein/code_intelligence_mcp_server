import { test, expect, afterEach } from "bun:test";
import { renderHook, act, waitFor } from "@testing-library/react";
import { useLogStream } from "@/features/logs/useLogStream";

class FakeEventSource {
  static last: FakeEventSource | null = null;
  onopen: (() => void) | null = null;
  onmessage: ((e: MessageEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  listeners: Record<string, (e: MessageEvent) => void> = {};
  url: string;
  constructor(url: string) {
    this.url = url;
    FakeEventSource.last = this;
  }
  addEventListener(type: string, cb: (e: MessageEvent) => void) {
    this.listeners[type] = cb;
  }
  close() {}
  emitOpen() {
    this.onopen?.();
  }
  emitMessage(data: string) {
    this.onmessage?.({ data } as MessageEvent);
  }
}

afterEach(() => {
  FakeEventSource.last = null;
});

test("useLogStream collects SSE lines and reports connected", async () => {
  (globalThis as unknown as { EventSource: unknown }).EventSource = FakeEventSource as unknown;

  const { result } = renderHook(() => useLogStream());
  await waitFor(() => expect(FakeEventSource.last).not.toBeNull());

  act(() => {
    FakeEventSource.last!.emitOpen();
    FakeEventSource.last!.emitMessage("first log line");
    FakeEventSource.last!.emitMessage("second log line");
  });

  await waitFor(() => expect(result.current.lines.length).toBe(2));
  expect(result.current.connected).toBe(true);
  expect(result.current.lines[0]!.text).toBe("first log line");
});
