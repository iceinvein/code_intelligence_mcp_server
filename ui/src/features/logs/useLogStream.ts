import { useEffect, useRef, useState } from "react";

export type LogLine = { id: number; text: string; lagged: boolean };

const MAX_LINES = 500;

export function useLogStream(): { lines: LogLine[]; connected: boolean } {
  const [lines, setLines] = useState<LogLine[]>([]);
  const [connected, setConnected] = useState(false);
  const counter = useRef(0);

  useEffect(() => {
    let source: EventSource | null = null;
    let retry: ReturnType<typeof setTimeout> | null = null;
    let closed = false;

    const push = (text: string, lagged: boolean) => {
      counter.current += 1;
      const line = { id: counter.current, text, lagged };
      setLines((prev) => {
        const next = prev.length >= MAX_LINES ? prev.slice(prev.length - MAX_LINES + 1) : prev;
        return [...next, line];
      });
    };

    const connect = () => {
      source = new EventSource("/api/logs/stream");
      source.onopen = () => setConnected(true);
      source.onmessage = (e: MessageEvent) => push(String(e.data), false);
      source.addEventListener("lagged", (e) => push(`[${String((e as MessageEvent).data)}]`, true));
      source.onerror = () => {
        setConnected(false);
        source?.close();
        if (!closed) retry = setTimeout(connect, 2000);
      };
    };

    connect();
    return () => {
      closed = true;
      if (retry) clearTimeout(retry);
      source?.close();
    };
  }, []);

  return { lines, connected };
}
