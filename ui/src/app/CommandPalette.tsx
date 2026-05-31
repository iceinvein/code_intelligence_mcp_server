import { Command } from "cmdk";
import { useEffect, useState } from "react";
import { useNavigate } from "react-router";

const ITEMS = [
  { label: "Go to Repositories", to: "/repos" },
  { label: "Go to Search", to: "/search" },
  { label: "Go to Settings", to: "/settings" },
];

export function CommandPalette() {
  const [open, setOpen] = useState(false);
  const navigate = useNavigate();

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
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center bg-black/45 pt-[16vh]" onClick={() => setOpen(false)}>
      <Command
        className="w-[min(680px,80vw)] overflow-hidden rounded-lg border border-border bg-popover text-popover-foreground"
        onClick={(e) => e.stopPropagation()}
      >
        <Command.Input autoFocus placeholder="type a command…" className="w-full border-b border-border bg-transparent px-4 py-3 text-sm outline-none" />
        <Command.List className="max-h-[50vh] overflow-y-auto p-1">
          <Command.Empty className="p-4 text-xs text-muted-foreground">no results</Command.Empty>
          {ITEMS.map((item) => (
            <Command.Item
              key={item.to}
              onSelect={() => {
                navigate(item.to);
                setOpen(false);
              }}
              className="cursor-pointer rounded px-3 py-2 text-xs data-[selected=true]:bg-primary/15"
            >
              {item.label}
            </Command.Item>
          ))}
        </Command.List>
      </Command>
    </div>
  );
}
