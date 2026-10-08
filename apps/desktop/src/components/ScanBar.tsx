import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Loader2, Pause, Play, Radar, Square } from "lucide-react";
import { useEffect } from "react";

import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import * as api from "@/lib/api";
import { useAppStore } from "@/lib/store";
import type { ScanReport } from "@/lib/types";
import { formatBytes, shortenPath } from "@/lib/utils";

/**
 * The scan control strip.
 *
 * Progress is derived from the event stream rather than polled, and the
 * "scanners finished" count is computed here from terminal events — the
 * backend deliberately does not send a running total, so the bar can never
 * jump backwards when concurrent scanners interleave.
 */
export function ScanBar({
  report,
  onReport,
}: {
  report: ScanReport | undefined;
  onReport: (report: ScanReport) => void;
}) {
  const queryClient = useQueryClient();
  const { running, paused, progress, finished, total, beginScan, applyProgress, endScan, setPaused, clearSelection } =
    useAppStore();

  // One subscription for the app's lifetime; events are ignored when idle.
  useEffect(() => {
    const unlisten = api.onScanProgress(applyProgress);
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [applyProgress]);

  const scan = useMutation({
    mutationFn: () => api.startScan(),
    onMutate: () => {
      clearSelection();
      beginScan();
    },
    onSuccess: (result) => {
      onReport(result);
      // History and disk figures changed as a side effect of the scan.
      void queryClient.invalidateQueries({ queryKey: ["scanHistory"] });
      void queryClient.invalidateQueries({ queryKey: ["diskTrend"] });
      void queryClient.invalidateQueries({ queryKey: ["explanation"] });
    },
    onSettled: endScan,
  });

  const active = Object.values(progress).filter((event) => event.phase === "scanning");
  const current = active[active.length - 1];
  const fraction = total > 0 ? finished.size / total : 0;
  const found = Object.values(progress).reduce((sum, event) => sum + event.bytesFound, 0);

  return (
    <div className="flex items-center gap-4 border-b border-border bg-card px-6 py-3">
      {running ? (
        <>
          <Button
            variant="outline"
            onClick={() => {
              if (paused) void api.resumeScan();
              else void api.pauseScan();
              setPaused(!paused);
            }}
          >
            {paused ? <Play /> : <Pause />}
            {paused ? "Resume" : "Pause"}
          </Button>
          <Button variant="outline" onClick={() => void api.cancelScan()}>
            <Square />
            Stop
          </Button>
        </>
      ) : (
        <Button onClick={() => scan.mutate()} disabled={scan.isPending}>
          {scan.isPending ? <Loader2 className="animate-spin" /> : <Radar />}
          {report ? "Scan again" : "Scan my disk"}
        </Button>
      )}

      <div className="min-w-0 flex-1">
        {running ? (
          <div className="space-y-1.5">
            <Progress value={Math.round(fraction * 100)} className="gap-1.5" />
            <p className="truncate text-xs text-muted-foreground" aria-live="polite">
              {paused
                ? "Paused"
                : current?.currentPath
                  ? `Scanning ${shortenPath(current.currentPath)}`
                  : "Starting scan…"}
              {found > 0 ? ` · ${formatBytes(found)} found` : ""}
              {total > 0 ? ` · ${finished.size} of ${total} scanners done` : ""}
            </p>
          </div>
        ) : report ? (
          <p className="truncate text-xs text-muted-foreground">
            Last scan looked at {report.scanners.length} places and found{" "}
            {formatBytes(report.summary.totalSize)}
            {report.cancelled ? " before you stopped it" : ""}.
          </p>
        ) : (
          <p className="truncate text-xs text-muted-foreground">
            Nothing is read outside your home folder, and nothing ever leaves this machine.
          </p>
        )}
      </div>
    </div>
  );
}
