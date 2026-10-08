import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { History, Undo2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import * as api from "@/lib/api";
import { formatBytes, formatDateTime, formatDuration, plural } from "@/lib/utils";

/**
 * Cleanup and scan history.
 *
 * Every cleanup is recorded before the user can forget what they did, and undo
 * is offered only where the platform can genuinely deliver it — on macOS the
 * Trash has no programmatic restore, so we say so instead of showing a button
 * that fails.
 */
export function HistoryView() {
  const queryClient = useQueryClient();
  const cleanups = useQuery({ queryKey: ["cleanupHistory"], queryFn: () => api.cleanupHistory(50) });
  const scans = useQuery({ queryKey: ["scanHistory"], queryFn: () => api.scanHistory(20) });
  const canUndo = useQuery({ queryKey: ["undoSupported"], queryFn: api.undoSupported });

  const undo = useMutation({
    mutationFn: (id: string) => api.undoCleanup(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["cleanupHistory"] });
      void queryClient.invalidateQueries({ queryKey: ["totalFreed"] });
    },
  });

  return (
    <div className="space-y-5">
      <Card>
        <CardHeader>
          <CardTitle>Cleanups</CardTitle>
          <CardDescription>
            Everything SpaceKeeper has removed, with what it freed.
            {canUndo.data === false &&
              " This platform cannot restore from the Trash automatically — open the Trash and choose “Put Back”."}
          </CardDescription>
        </CardHeader>
        <CardContent>
          {cleanups.isLoading ? (
            <Skeleton className="h-24 w-full" />
          ) : cleanups.data?.length ? (
            <ul className="divide-y divide-border">
              {cleanups.data.map((record) => (
                <li key={record.id} className="flex items-center gap-4 py-3">
                  <div className="min-w-0 flex-1">
                    <p className="font-medium">
                      {record.mode === "trash" ? "Moved to Trash" : "Deleted permanently"} ·{" "}
                      {formatBytes(record.freed)}
                    </p>
                    <p className="text-sm text-muted-foreground">
                      {formatDateTime(record.performedAt)} · {plural(record.removed, "item")}
                      {record.skipped > 0 ? ` · ${record.skipped} skipped` : ""}
                      {record.failed > 0 ? ` · ${record.failed} failed` : ""}
                    </p>
                  </div>
                  {record.undoable && (
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => undo.mutate(record.id)}
                      disabled={undo.isPending}
                    >
                      <Undo2 />
                      Undo
                    </Button>
                  )}
                </li>
              ))}
            </ul>
          ) : (
            <Empty className="border-none py-8">
              <EmptyHeader>
                <EmptyMedia variant="icon">
                  <History />
                </EmptyMedia>
                <EmptyTitle>Nothing removed yet</EmptyTitle>
                <EmptyDescription>
                  Cleanups appear here so you always know what was removed and when.
                </EmptyDescription>
              </EmptyHeader>
            </Empty>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Scans</CardTitle>
          <CardDescription>Past scans and what they found.</CardDescription>
        </CardHeader>
        <CardContent>
          {scans.isLoading ? (
            <Skeleton className="h-24 w-full" />
          ) : scans.data?.length ? (
            <ul className="divide-y divide-border text-sm">
              {scans.data.map((scan) => (
                <li key={scan.id} className="flex items-center justify-between gap-4 py-2.5">
                  <span className="text-muted-foreground">{formatDateTime(scan.startedAt)}</span>
                  <span className="tabular-nums">
                    {formatBytes(scan.totalSize)} found · {formatBytes(scan.safeSize)} safe ·{" "}
                    {formatDuration(scan.durationMs)}
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="py-6 text-center text-sm text-muted-foreground">No scans recorded yet.</p>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
