import { Monitor, Moon, Sun } from "lucide-react";
import { useEffect, useState } from "react";

import { Dashboard } from "@/components/Dashboard";
import { ExplorerView } from "@/components/ExplorerView";
import { HistoryView } from "@/components/HistoryView";
import { ResultsView } from "@/components/ResultsView";
import { ScanBar } from "@/components/ScanBar";
import { SettingsView } from "@/components/SettingsView";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { applyTheme, useAppStore, watchSystemTheme } from "@/lib/store";
import type { ScanReport } from "@/lib/types";
import { plural } from "@/lib/utils";

export default function App() {
  const [report, setReport] = useState<ScanReport | undefined>();
  const [tab, setTab] = useState("dashboard");
  const { theme, setTheme } = useAppStore();

  useEffect(() => {
    applyTheme(theme);
    return watchSystemTheme();
  }, [theme]);

  return (
    <div className="flex h-screen flex-col bg-background">
      <header className="flex items-center gap-4 border-b border-border bg-card px-6 py-3">
        <div className="flex items-center gap-2.5">
          <Logo />
          <div>
            <h1 className="text-sm font-semibold leading-none">SpaceKeeper</h1>
            <p className="text-xs text-muted-foreground">Understand why your disk is full</p>
          </div>
        </div>

        <Tabs value={tab} onValueChange={setTab} className="ml-6">
          <TabsList>
            <TabsTrigger value="dashboard">Dashboard</TabsTrigger>
            <TabsTrigger value="results">
              Results
              {report ? (
                <span className="ml-1.5 rounded-full bg-muted px-1.5 text-xs tabular-nums">
                  {report.summary.totalItems}
                </span>
              ) : null}
            </TabsTrigger>
            <TabsTrigger value="explore">Explore</TabsTrigger>
            <TabsTrigger value="history">History</TabsTrigger>
            <TabsTrigger value="settings">Settings</TabsTrigger>
          </TabsList>
        </Tabs>

        <div className="ml-auto">
          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={() =>
                    setTheme(theme === "system" ? "light" : theme === "light" ? "dark" : "system")
                  }
                  aria-label={`Theme: ${theme}. Click to change.`}
                />
              }
            >
              {theme === "system" ? <Monitor /> : theme === "light" ? <Sun /> : <Moon />}
            </TooltipTrigger>
            <TooltipContent>Theme: {theme}</TooltipContent>
          </Tooltip>
        </div>
      </header>

      <ScanBar report={report} onReport={setReport} />

      {/* One Tabs root drives both the header triggers and the panels, so
          keyboard users get proper roving focus and arrow-key navigation. */}
      <Tabs value={tab} onValueChange={setTab} className="min-h-0 flex-1">
        <main className="h-full min-h-0 overflow-y-auto px-6 py-5">
          <TabsContent value="dashboard">
            <Dashboard report={report} onShowResults={() => setTab("results")} />
          </TabsContent>
          <TabsContent value="results" className="h-full">
            <ResultsView report={report} />
          </TabsContent>
          <TabsContent value="explore" className="h-full">
            <ExplorerView />
          </TabsContent>
          <TabsContent value="history">
            <HistoryView />
          </TabsContent>
          <TabsContent value="settings">
            <SettingsView />
          </TabsContent>
        </main>
      </Tabs>

      <footer className="flex items-center gap-3 border-t border-border bg-card px-6 py-2 text-xs text-muted-foreground">
        <span>Everything runs on this machine. Nothing is uploaded.</span>
        {report ? (
          <span className="ml-auto tabular-nums">
            {plural(report.results.length, "result")} from {report.scanners.length} scanners
          </span>
        ) : null}
      </footer>
    </div>
  );
}

function Logo() {
  return (
    <svg width="28" height="28" viewBox="0 0 24 24" aria-hidden className="shrink-0">
      <rect width="24" height="24" rx="6" className="fill-primary" />
      <circle
        cx="12"
        cy="12"
        r="6.5"
        fill="none"
        strokeWidth="3"
        strokeLinecap="round"
        strokeDasharray="32 41"
        transform="rotate(-90 12 12)"
        className="stroke-primary-foreground"
      />
      <circle cx="12" cy="12" r="2.4" className="fill-primary-foreground" />
    </svg>
  );
}
