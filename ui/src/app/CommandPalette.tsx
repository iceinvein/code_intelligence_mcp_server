import { Command } from "cmdk";
import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { ArrowRight, Monitor, Moon, Sun, type LucideIcon } from "lucide-react";
import { useTheme, type Theme } from "@/lib/theme";

const NAV = [
  { label: "overview", to: "/" },
  { label: "search", to: "/search" },
  { label: "repositories", to: "/repos" },
  { label: "graph", to: "/graph" },
  { label: "symbols", to: "/symbols" },
  { label: "settings", to: "/settings" },
  { label: "consent", to: "/consent" },
  { label: "logs", to: "/logs" },
  { label: "jobs · sessions", to: "/activity" },
];

const THEMES: { label: string; value: Theme; icon: LucideIcon }[] = [
  { label: "light", value: "light", icon: Sun },
  { label: "dark", value: "dark", icon: Moon },
  { label: "system", value: "system", icon: Monitor },
];

export function CommandPalette() {
  const [open, setOpen] = useState(false);
  const navigate = useNavigate();
  const { setTheme } = useTheme();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "k" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        setOpen((o) => !o);
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  if (!open) return null;

  const run = (fn: () => void) => {
    fn();
    setOpen(false);
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-[oklch(20%_0.025_250/0.45)] pt-[16vh]"
      onClick={() => setOpen(false)}
    >
      <Command
        label="Command palette"
        className="w-[min(620px,90vw)] overflow-hidden rounded-lg border border-border bg-popover text-popover-foreground shadow-[0_12px_40px_-16px_oklch(20%_0.02_250/0.45)]"
        onClick={(e) => e.stopPropagation()}
      >
        <Command.Input
          autoFocus
          placeholder="search commands…"
          className="w-full border-b border-border bg-transparent px-4 py-3 text-sm outline-none placeholder:text-muted-foreground"
        />
        <Command.List className="max-h-[52vh] overflow-y-auto p-1.5">
          <Command.Empty className="px-3 py-6 text-center text-xs text-muted-foreground">
            no matching commands
          </Command.Empty>
          <Command.Group
            heading="Navigate"
            className="[&_[cmdk-group-heading]]:px-2.5 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-[0.625rem] [&_[cmdk-group-heading]]:font-medium [&_[cmdk-group-heading]]:uppercase [&_[cmdk-group-heading]]:tracking-[0.12em] [&_[cmdk-group-heading]]:text-label"
          >
            {NAV.map((item) => (
              <Command.Item
                key={item.to}
                value={`go to ${item.label}`}
                onSelect={() => run(() => navigate(item.to))}
                className="flex cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-2 text-sm data-[selected=true]:bg-muted data-[selected=true]:text-foreground"
              >
                <ArrowRight className="h-3.5 w-3.5 text-muted-foreground" aria-hidden />
                {item.label}
              </Command.Item>
            ))}
          </Command.Group>
          <Command.Group
            heading="Theme"
            className="mt-1 [&_[cmdk-group-heading]]:px-2.5 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-[0.625rem] [&_[cmdk-group-heading]]:font-medium [&_[cmdk-group-heading]]:uppercase [&_[cmdk-group-heading]]:tracking-[0.12em] [&_[cmdk-group-heading]]:text-label"
          >
            {THEMES.map((t) => {
              const Icon = t.icon;
              return (
                <Command.Item
                  key={t.value}
                  value={`theme ${t.label}`}
                  onSelect={() => run(() => setTheme(t.value))}
                  className="flex cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-2 text-sm data-[selected=true]:bg-muted data-[selected=true]:text-foreground"
                >
                  <Icon className="h-3.5 w-3.5 text-muted-foreground" aria-hidden />
                  {t.label}
                </Command.Item>
              );
            })}
          </Command.Group>
        </Command.List>
      </Command>
    </div>
  );
}
