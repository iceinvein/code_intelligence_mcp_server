import { lazy, Suspense } from "react";
import { createBrowserRouter, Navigate } from "react-router";
import { Shell } from "@/app/Shell";
import { OverviewView } from "@/features/overview/OverviewView";
import { ReposView } from "@/features/repos/ReposView";
import { ActivityView } from "@/features/activity/ActivityView";
import { ConsentView } from "@/features/consent/ConsentView";
import { LogsView } from "@/features/logs/LogsView";
import { SearchView } from "@/features/search/SearchView";
import { SettingsView } from "@/features/settings/SettingsView";
import { SymbolsView } from "@/features/symbols/SymbolsView";
import { UsageView } from "@/features/usage/UsageView";

const GraphView = lazy(() =>
  import("@/features/graph/GraphView").then((m) => ({ default: m.GraphView })),
);

export const router = createBrowserRouter([
  {
    path: "/",
    element: <Shell />,
    children: [
      { index: true, element: <OverviewView /> },
      { path: "repos", element: <ReposView /> },
      { path: "search", element: <SearchView /> },
      {
        path: "graph",
        element: (
          <Suspense fallback={<div className="text-xs text-muted-foreground">loading graph...</div>}>
            <GraphView />
          </Suspense>
        ),
      },
      { path: "symbols", element: <SymbolsView /> },
      { path: "settings", element: <SettingsView /> },
      { path: "consent", element: <ConsentView /> },
      { path: "logs", element: <LogsView /> },
      { path: "activity", element: <ActivityView /> },
      { path: "usage", element: <UsageView /> },
      { path: "*", element: <Navigate to="/" replace /> },
    ],
  },
]);
