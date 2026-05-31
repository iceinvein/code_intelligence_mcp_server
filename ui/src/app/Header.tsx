import { useQuery } from "@tanstack/react-query";
import { fetchStatus } from "@/api/repos";
import { useTheme } from "@/lib/theme";
import { Button } from "@/components/ui/button";

export function Header() {
  const { data, isError } = useQuery({
    queryKey: ["status"],
    queryFn: ({ signal }) => fetchStatus(signal),
    refetchInterval: 5_000,
  });
  const { theme, setTheme } = useTheme();
  const next = theme === "dark" ? "light" : theme === "light" ? "system" : "dark";

  return (
    <header className="flex items-center gap-3 border-b border-border px-4 py-3">
      <span className="font-serif italic text-base">code intelligence</span>
      <span className="text-[11px] text-muted-foreground">127.0.0.1:17802</span>
      <div className="flex-1" />
      {data ? (
        <span className="text-[11px] text-muted-foreground">v{data.version}</span>
      ) : null}
      <Button variant="outline" size="sm" onClick={() => setTheme(next)}>
        {theme}
      </Button>
      <span
        role="status"
        aria-label={isError ? "daemon unreachable" : "daemon healthy"}
        title={isError ? "daemon unreachable" : "daemon healthy"}
        className={cnPulse(isError)}
      />
    </header>
  );
}

function cnPulse(isError: boolean): string {
  return isError
    ? "h-2 w-2 rounded-full border border-destructive"
    : "h-2 w-2 rounded-full bg-primary shadow-[0_0_0_3px_color-mix(in_srgb,var(--color-primary)_22%,transparent)]";
}
