import { createBrowserRouter, Navigate } from "react-router";
import { Shell } from "@/app/Shell";
import { ReposView } from "@/features/repos/ReposView";
import { ActivityView } from "@/features/activity/ActivityView";
import { ConsentView } from "@/features/consent/ConsentView";
import { LogsView } from "@/features/logs/LogsView";
import { SearchView } from "@/features/search/SearchView";
import { SettingsView } from "@/features/settings/SettingsView";
import { SymbolsView } from "@/features/symbols/SymbolsView";

function Placeholder({ name }: { name: string }) {
  return <div className="text-xs text-muted-foreground">{name}: coming in a later phase</div>;
}

export const router = createBrowserRouter([
  {
    path: "/",
    element: <Shell />,
    children: [
      { index: true, element: <Navigate to="/repos" replace /> },
      { path: "repos", element: <ReposView /> },
      { path: "search", element: <SearchView /> },
      { path: "graph", element: <Placeholder name="graph" /> },
      { path: "symbols", element: <SymbolsView /> },
      { path: "settings", element: <SettingsView /> },
      { path: "consent", element: <ConsentView /> },
      { path: "logs", element: <LogsView /> },
      { path: "activity", element: <ActivityView /> },
      { path: "*", element: <Navigate to="/repos" replace /> },
    ],
  },
]);
