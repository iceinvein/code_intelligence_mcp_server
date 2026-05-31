import { createBrowserRouter, Navigate } from "react-router";
import { Shell } from "@/app/Shell";
import { ReposView } from "@/features/repos/ReposView";
import { ActivityView } from "@/features/activity/ActivityView";

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
      { path: "search", element: <Placeholder name="search" /> },
      { path: "graph", element: <Placeholder name="graph" /> },
      { path: "symbols", element: <Placeholder name="symbols" /> },
      { path: "settings", element: <Placeholder name="settings" /> },
      { path: "consent", element: <Placeholder name="consent" /> },
      { path: "logs", element: <Placeholder name="logs" /> },
      { path: "activity", element: <ActivityView /> },
      { path: "*", element: <Navigate to="/repos" replace /> },
    ],
  },
]);
