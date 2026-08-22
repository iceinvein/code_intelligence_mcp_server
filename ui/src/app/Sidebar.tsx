import { NavLink } from "react-router";
import {
  Activity,
  Boxes,
  Gauge,
  Hash,
  LayoutDashboard,
  ScrollText,
  Search,
  Settings,
  Share2,
  ShieldCheck,
  type LucideIcon,
} from "lucide-react";
import { useConsent } from "@/features/consent/useConsent";
import { cn } from "@/lib/utils";

type NavItem = { to: string; label: string; icon: LucideIcon; end?: boolean };

const NAV: NavItem[] = [
  { to: "/", label: "overview", icon: LayoutDashboard, end: true },
  { to: "/search", label: "search", icon: Search },
  { to: "/repos", label: "repositories", icon: Boxes },
  { to: "/symbols", label: "symbols", icon: Hash },
  { to: "/graph", label: "graph", icon: Share2 },
  { to: "/consent", label: "consent", icon: ShieldCheck },
  { to: "/logs", label: "logs", icon: ScrollText },
  { to: "/activity", label: "jobs · sessions", icon: Activity },
  { to: "/usage", label: "usage", icon: Gauge },
  { to: "/settings", label: "settings", icon: Settings },
];

export function Sidebar() {
  const consent = useConsent();
  const pendingCount = consent.data?.pending.length ?? 0;

  return (
    <nav
      aria-label="Primary"
      className="w-14 shrink-0 border-r border-border px-2 py-3 sm:w-48"
    >
      <div className="hidden px-2.5 pb-2 text-[0.625rem] font-medium uppercase tracking-[0.13em] text-label sm:block">
        navigate
      </div>
      <ul className="flex flex-col gap-0.5">
        {NAV.map((item) => {
          const Icon = item.icon;
          const showBadge = item.to === "/consent" && pendingCount > 0;
          return (
            <li key={item.to}>
              <NavLink
                to={item.to}
                end={item.end}
                title={item.label}
                className={({ isActive }) =>
                  cn(
                    "flex items-center justify-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm transition-colors duration-150 sm:justify-start",
                    isActive
                      ? "bg-primary/10 font-medium text-primary"
                      : "text-muted-foreground hover:bg-muted hover:text-foreground",
                  )
                }
              >
                <span className="relative shrink-0">
                  <Icon className="h-3.5 w-3.5" aria-hidden />
                  {showBadge ? (
                    <span
                      aria-hidden
                      className="absolute -right-1 -top-1 h-1.5 w-1.5 rounded-full bg-primary sm:hidden"
                    />
                  ) : null}
                </span>
                <span className="hidden min-w-0 flex-1 truncate sm:block">{item.label}</span>
                {showBadge ? (
                  <span
                    aria-label={`${pendingCount} pending`}
                    className="hidden h-4 min-w-4 items-center justify-center rounded-full bg-primary px-1 font-mono text-[0.625rem] leading-none text-primary-foreground sm:inline-flex"
                  >
                    {pendingCount}
                  </span>
                ) : null}
              </NavLink>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}
