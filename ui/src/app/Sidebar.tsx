import { NavLink } from "react-router";
import { useConsent } from "@/features/consent/useConsent";
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
  const consent = useConsent();
  const pendingCount = consent.data?.pending.length ?? 0;

  return (
    <nav className="w-44 shrink-0 border-r border-border py-3">
      <div className="px-4 pb-2 text-[9px] uppercase tracking-[0.18em] text-label">navigate</div>
      {NAV.map((item) => (
        <NavLink
          key={item.to}
          to={item.to}
          className={({ isActive }) =>
            cn(
              "flex items-center justify-between border-l-2 px-4 py-1.5 text-xs",
              isActive
                ? "border-primary bg-primary/10 text-foreground"
                : "border-transparent text-muted-foreground hover:text-foreground",
            )
          }
        >
          <span>{item.label}</span>
          {item.to === "/consent" && pendingCount > 0 ? (
            <span
              aria-label={`${pendingCount} pending`}
              className="rounded-full bg-primary px-1.5 text-[10px] leading-4 text-primary-foreground"
            >
              {pendingCount}
            </span>
          ) : null}
        </NavLink>
      ))}
    </nav>
  );
}
