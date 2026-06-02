import { Outlet } from "react-router";
import { Sidebar } from "@/app/Sidebar";
import { Header } from "@/app/Header";
import { CommandPalette } from "@/app/CommandPalette";

export function Shell() {
  return (
    <div className="flex h-full flex-col">
      <Header />
      <div className="flex min-h-0 flex-1">
        <Sidebar />
        <main className="min-w-0 flex-1 overflow-y-auto px-5 py-6 sm:px-7 sm:py-7">
          <div className="mx-auto max-w-[1080px]">
            <Outlet />
          </div>
        </main>
      </div>
      <CommandPalette />
    </div>
  );
}
