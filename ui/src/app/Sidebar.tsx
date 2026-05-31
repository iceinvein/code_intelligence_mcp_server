import { NavLink } from "react-router";
import { cn } from "@/lib/utils";

const NAV = [
  { to: "/search", label: "search" },
  { to: "/repos", label: "repositories" },
  { to: "/graph", label: "graph" },
  { to: "/symbols", label: "symbols" },
  { to: "/settings", label: "settings" },
  { to: "/consent", label: "consent" },
  { to: "/logs", label: "logs" },
  { to: "/activity", label: "jobs · sessions" },
];

export function Sidebar() {
  return (
    <nav className="w-44 shrink-0 border-r border-border py-3">
      <div className="px-4 pb-2 text-[9px] uppercase tracking-[0.18em] text-label">navigate</div>
      {NAV.map((item) => (
        <NavLink
          key={item.to}
          to={item.to}
          className={({ isActive }) =>
            cn(
              "block border-l-2 px-4 py-1.5 text-xs",
              isActive
                ? "border-primary bg-primary/10 text-foreground"
                : "border-transparent text-muted-foreground hover:text-foreground",
            )
          }
        >
          {item.label}
        </NavLink>
      ))}
    </nav>
  );
}
