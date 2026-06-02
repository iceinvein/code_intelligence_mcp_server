import { useQuery } from "@tanstack/react-query";
import { Monitor, Moon, Sun } from "lucide-react";
import { fetchStatus } from "@/api/repos";
import { useTheme, type Theme } from "@/lib/theme";
import { Button } from "@/components/ui/button";
import { StatusGlyph } from "@/components/ui/status";
import { formatUptime } from "@/lib/format";

export function Header() {
  const { data, isError, isLoading } = useQuery({
    queryKey: ["status"],
    queryFn: ({ signal }) => fetchStatus(signal),
    refetchInterval: 5_000,
  });
  const { theme, setTheme } = useTheme();
  const next: Theme = theme === "dark" ? "light" : theme === "light" ? "system" : "dark";
  const ThemeIcon = theme === "dark" ? Moon : theme === "light" ? Sun : Monitor;

  const health = isError ? "fail" : isLoading && !data ? "idle" : "ok";
  const healthLabel = isError ? "unreachable" : isLoading && !data ? "connecting" : "healthy";

  return (
    <header className="flex items-center gap-3 border-b border-border px-4 py-2.5 sm:px-6">
      <span className="font-serif text-[1.0625rem] italic tracking-tight">code intelligence</span>
      <span className="hidden font-mono text-[0.6875rem] text-muted-foreground sm:inline">
        127.0.0.1:17802
      </span>

      <div className="flex-1" />

      {data ? (
        <span className="hidden font-mono text-[0.6875rem] text-muted-foreground md:inline">
          v{data.version} · up {formatUptime(data.uptime_s)}
        </span>
      ) : null}

      <StatusGlyph
        state={health}
        label={healthLabel}
        className="text-[0.6875rem] font-medium"
      />

      <Button
        variant="outline"
        size="sm"
        onClick={() => setTheme(next)}
        aria-label={`theme: ${theme}. switch to ${next}`}
        title={`theme: ${theme}`}
      >
        <ThemeIcon className="h-3.5 w-3.5" aria-hidden />
        <span className="hidden lowercase sm:inline">{theme}</span>
      </Button>
    </header>
  );
}
