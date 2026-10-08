import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Loader2, Trash2 } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import * as api from "@/lib/api";
import { summarizeSelection, useAppStore } from "@/lib/store";
import type { CleanupMode, CleanupRecord, ScanReport } from "@/lib/types";
import { formatBytes, plural } from "@/lib/utils";

/**
 * The confirmation step.
 *
 * The numbers shown here come from a real dry run against the backend — the
 * same code path that performs the deletion, with `dryRun` set. That means the
 * dialog cannot promise something the cleanup would not do, and items the
 * backend intends to skip (protected, already gone) are visible *before* the
 * user commits rather than in a report afterwards.
 */
export function CleanupDialog({ report }: { report: ScanReport }) {
  const [open, setOpen] = useState(false);
  const [mode, setMode] = useState<CleanupMode>("trash");
  const [done, setDone] = useState<CleanupRecord | null>(null);

  const queryClient = useQueryClient();
  const { selection, clearSelection } = useAppStore();
  const summary = summarizeSelection(report, selection);
  const ids = [...selection];

  const preview = useQuery({
    queryKey: ["cleanupPreview", ids.join(","), mode],
    queryFn: () => api.previewCleanup(ids, mode),
    enabled: open && ids.length > 0 && !done,
  });

  const clean = useMutation({
    mutationFn: () => api.runCleanup(ids, mode),
    onSuccess: (record) => {
      setDone(record);
      clearSelection();
      void queryClient.invalidateQueries({ queryKey: ["cleanupHistory"] });
      void queryClient.invalidateQueries({ queryKey: ["totalFreed"] });
      void queryClient.invalidateQueries({ queryKey: ["disks"] });
    },
  });

  const skipped = preview.data?.items.filter((item) => item.status === "skipped") ?? [];

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) {
          setDone(null);
          clean.reset();
        }
      }}
    >
      <DialogTrigger
        render={
          <Button disabled={summary.count === 0}>
            <Trash2 />
            Clean up{summary.count > 0 ? ` ${formatBytes(summary.size)}` : ""}
          </Button>
        }
      />

      <DialogContent className="sm:max-w-lg">
        {done ? (
          <>
            <DialogHeader>
              <DialogTitle>Done</DialogTitle>
              <DialogDescription>
                {done.mode === "trash"
                  ? `Moved ${plural(done.removed, "item")} to the Trash, freeing ${formatBytes(done.freed)}.`
                  : `Permanently removed ${plural(done.removed, "item")}, freeing ${formatBytes(done.freed)}.`}
                {done.failed > 0 ? ` ${plural(done.failed, "item")} could not be removed.` : ""}
              </DialogDescription>
            </DialogHeader>

            {done.undoable && (
              <p className="px-6 text-sm text-muted-foreground">
                Everything went to the Trash, so you can put it back if you change your mind.
              </p>
            )}

            <DialogFooter>
              <DialogClose render={<Button>Close</Button>} />
            </DialogFooter>
          </>
        ) : (
          <>
            <DialogHeader>
              <DialogTitle>Review before cleaning up</DialogTitle>
              <DialogDescription>
                {preview.isLoading
                  ? "Checking what can be removed…"
                  : preview.data
                    ? `${plural(preview.data.removed, "item")} will be removed, freeing ${formatBytes(preview.data.freed)}.`
                    : `${plural(summary.count, "item")} selected.`}
              </DialogDescription>
            </DialogHeader>

            <div className="space-y-4 px-6">
              <div className="flex items-start gap-3 rounded-lg border border-border p-3">
                <Checkbox
                  id="permanent"
                  checked={mode === "permanent"}
                  onCheckedChange={(checked) => setMode(checked ? "permanent" : "trash")}
                  className="mt-0.5"
                />
                <div className="space-y-1">
                  <Label htmlFor="permanent" className="font-medium">
                    Delete permanently instead of using the Trash
                  </Label>
                  <p className="text-sm text-muted-foreground">
                    {mode === "permanent"
                      ? "This cannot be undone. Files will not be recoverable."
                      : "Recommended. Items go to the Trash and can be put back."}
                  </p>
                </div>
              </div>

              {summary.hasReview && (
                <p className="flex items-start gap-2 rounded-lg bg-warn/10 p-3 text-sm text-warn">
                  <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden />
                  Some selected items are not recreated automatically. Make sure you have looked at
                  them.
                </p>
              )}

              {skipped.length > 0 && (
                <div className="rounded-lg bg-muted p-3 text-sm">
                  <p className="mb-1 font-medium">
                    {plural(skipped.length, "item")} will be skipped
                  </p>
                  <ul className="space-y-0.5 text-muted-foreground">
                    {skipped.slice(0, 4).map((item) => (
                      <li key={item.id} className="truncate">
                        {item.path} — {item.message}
                      </li>
                    ))}
                  </ul>
                </div>
              )}

              {clean.isError && (
                <p className="rounded-lg bg-destructive/10 p-3 text-sm text-destructive">
                  {(clean.error as { message?: string })?.message ?? "The cleanup failed."}
                </p>
              )}
            </div>

            <DialogFooter>
              <DialogClose render={<Button variant="outline">Cancel</Button>} />
              <Button
                variant={mode === "permanent" ? "destructive" : "default"}
                onClick={() => clean.mutate()}
                disabled={clean.isPending || preview.isLoading || (preview.data?.removed ?? 0) === 0}
              >
                {clean.isPending ? <Loader2 className="animate-spin" /> : <Trash2 />}
                {mode === "permanent" ? "Delete permanently" : "Move to Trash"}
              </Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
