import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  ChevronRight,
  CornerLeftUp,
  ExternalLink,
  File as FileIcon,
  Folder,
  Link2,
  Loader2,
  Lock,
  RotateCw,
  Trash2,
} from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useState } from "react";

import { VirtualList } from "@/components/VirtualList";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useMeasuredSizes } from "@/hooks/useMeasuredSizes";
import * as api from "@/lib/api";
import type { BrowseEntry, CleanupRecord, Protection, SortBy } from "@/lib/types";
import { cn, formatAge, formatBytes, plural } from "@/lib/utils";

/**
 * Browse the disk by size.
 *
 * The scan answers "what is safe to delete?"; this answers "what is actually in
 * there?" — the question people ask when a scan has cleaned everything obvious
 * and the disk is still full.
 *
 * Every row carries its protection state, so the reason something cannot be
 * removed is visible in place rather than discovered by clicking and failing.
 */
export function ExplorerView() {
  const queryClient = useQueryClient();
  const [path, setPath] = useState<string | undefined>();
  const [sort, setSort] = useState<SortBy>("size");
  const [selected, setSelected] = useState<Set<string>>(new Set());

  // Sorting is not in the key: the listing arrives before its sizes do, so
  // ordering happens client-side as measurements stream in. Re-fetching the
  // folder just to reorder it would throw away work already done.
  const listing = useQuery({
    queryKey: ["browse", path ?? "~"],
    queryFn: () => api.browseDirectory(path),
    // Listing is now fast, but keeping the previous folder on screen still
    // avoids a flash when stepping between directories.
    placeholderData: (previous) => previous,
  });

  const { entries, totalSize, measuring } = useMeasuredSizes(listing.data, sort);

  // Moving to a different folder invalidates any selection made in the old one.
  const navigate = useCallback((next: string) => {
    setSelected(new Set());
    setPath(next);
  }, []);

  // Backspace goes up a level, matching every file manager.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target && ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName)) return;
      if (event.key === "Backspace" && listing.data?.parent) {
        event.preventDefault();
        navigate(listing.data.parent);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [listing.data?.parent, navigate]);

  // Stable across renders, so a memoised row does not re-render every time
  // some other row's checkbox changes.
  const toggle = useCallback((entryPath: string) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(entryPath)) next.delete(entryPath);
      else next.add(entryPath);
      return next;
    });
  }, []);

  const open = useCallback(
    (entryPath: string) => navigate(entryPath),
    [navigate],
  );

  const refresh = () => {
    setSelected(new Set());
    void queryClient.invalidateQueries({ queryKey: ["browse"] });
  };

  const chosen = useMemo(
    () => entries.filter((entry) => selected.has(entry.path)),
    [entries, selected],
  );

  return (
    <div className="flex h-full flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="outline"
          size="icon"
          disabled={!listing.data?.parent}
          onClick={() => listing.data?.parent && navigate(listing.data.parent)}
          aria-label="Go up one folder (Backspace)"
        >
          <CornerLeftUp />
        </Button>

        <nav aria-label="Folder path" className="flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto">
          {listing.data?.breadcrumbs.map((crumb, index) => (
            <span key={crumb.path} className="flex shrink-0 items-center gap-0.5">
              {index > 0 && (
                <ChevronRight className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
              )}
              <button
                type="button"
                onClick={() => navigate(crumb.path)}
                aria-current={index === (listing.data?.breadcrumbs.length ?? 0) - 1 ? "page" : undefined}
                className={cn(
                  "rounded-md px-1.5 py-1 text-sm transition-colors",
                  "focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/50",
                  index === (listing.data?.breadcrumbs.length ?? 0) - 1
                    ? "font-medium text-foreground"
                    : "text-muted-foreground hover:bg-muted hover:text-foreground",
                )}
              >
                {index === 0 ? "Home" : crumb.name}
              </button>
            </span>
          ))}
        </nav>

        <NativeSelect
          value={sort}
          onChange={(event) => setSort(event.currentTarget.value as SortBy)}
          aria-label="Sort folder contents"
          className="w-40"
        >
          <option value="size">Largest first</option>
          <option value="name">Name</option>
          <option value="modified">Recently changed</option>
          <option value="oldest">Least recently used</option>
        </NativeSelect>

        <Button variant="outline" size="icon" onClick={refresh} aria-label="Refresh this folder">
          <RotateCw className={cn(listing.isFetching && "animate-spin")} />
        </Button>

        <DeleteSelectedDialog
          entries={chosen}
          onDone={() => {
            setSelected(new Set());
            void queryClient.invalidateQueries({ queryKey: ["browse"] });
            void queryClient.invalidateQueries({ queryKey: ["disks"] });
            void queryClient.invalidateQueries({ queryKey: ["cleanupHistory"] });
            void queryClient.invalidateQueries({ queryKey: ["totalFreed"] });
          }}
        />
      </div>

      <ExplorerLegend />

      {listing.data?.protection.kind === "caution" && (
        <p className="flex items-start gap-2 rounded-lg bg-warn/10 px-3 py-2 text-sm text-warn">
          <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden />
          This folder holds {listing.data.protection.reason}. SpaceKeeper never cleans it
          automatically — anything removed here is your decision.
        </p>
      )}

      <Card className="flex min-h-0 flex-1 flex-col overflow-hidden">
        <div className="flex items-center justify-between gap-4 border-b border-border px-4 py-2 text-xs text-muted-foreground">
          <span>
            {listing.isLoading ? (
              "Reading folder…"
            ) : (
              <span className="flex items-center gap-2">
                {plural(entries.length, "item")} · {formatBytes(totalSize)}
                {measuring && (
                  <span className="flex items-center gap-1 text-muted-foreground">
                    <Loader2 className="size-3 animate-spin" aria-hidden />
                    still measuring
                  </span>
                )}
              </span>
            )}
          </span>
          {(listing.data?.hiddenEntries ?? 0) > 0 && (
            <span>{plural(listing.data?.hiddenEntries ?? 0, "smaller item")} not shown</span>
          )}
        </div>

        <CardContent className="flex min-h-0 flex-1 flex-col p-0">
          {listing.isLoading ? (
            <div className="space-y-2 p-4">
              {Array.from({ length: 8 }, (_, index) => (
                <Skeleton key={index} className="h-10 w-full" />
              ))}
            </div>
          ) : listing.isError ? (
            <p className="p-6 text-sm text-destructive">
              {(listing.error as { message?: string })?.message ?? "That folder could not be read."}
            </p>
          ) : entries.length === 0 ? (
            <p className="p-10 text-center text-sm text-muted-foreground">This folder is empty.</p>
          ) : (
            <VirtualList
              items={entries}
              estimateHeight={56}
              getKey={(entry) => entry.path}
              renderRow={(entry) => (
                <EntryRow
                  entry={entry}
                  checked={selected.has(entry.path)}
                  onToggle={toggle}
                  onOpen={open}
                />
              )}
            />
          )}
        </CardContent>
      </Card>
    </div>
  );
}

/**
 * One row.
 *
 * Memoised, and given callbacks that take the path rather than closing over
 * it, so ticking one checkbox re-renders one row instead of a thousand.
 */
const EntryRow = memo(function EntryRow({
  entry,
  checked,
  onToggle,
  onOpen,
}: {
  entry: BrowseEntry;
  checked: boolean;
  onToggle: (path: string) => void;
  onOpen: (path: string) => void;
}) {
  const deletable = !entry.isSymlink && entry.protection.kind !== "refused";
  const Icon = entry.isSymlink ? Link2 : entry.kind === "directory" ? Folder : FileIcon;

  return (
    <div
      className={cn(
        "group flex items-center gap-3 border-b border-border px-4 py-2 hover:bg-muted/40",
        !deletable && "bg-muted/20",
      )}
    >
      {/* A disabled checkbox looks almost identical to an enabled one, so a
          user clicks it and nothing happens. Showing the lock in the same slot
          says "not selectable" before the click rather than after it. */}
      {deletable ? (
        <Checkbox
          checked={checked}
          onCheckedChange={() => onToggle(entry.path)}
          aria-label={`Select ${entry.name}`}
        />
      ) : (
        <Tooltip>
          <TooltipTrigger
            render={
              <span
                tabIndex={0}
                className="flex size-4 cursor-help items-center justify-center text-muted-foreground"
              />
            }
          >
            <Lock className="size-3.5" aria-label={`${entry.name} cannot be selected`} />
          </TooltipTrigger>
          <TooltipContent>
            {entry.isSymlink
              ? "This is a shortcut to somewhere else. SpaceKeeper never removes these."
              : `SpaceKeeper will never remove this: ${
                  entry.protection.kind === "refused" ? entry.protection.reason : "it is protected"
                }.`}
          </TooltipContent>
        </Tooltip>
      )}

      <Icon
        className={cn(
          "size-4 shrink-0",
          !deletable
            ? "text-muted-foreground/60"
            : entry.kind === "directory"
              ? "text-primary"
              : "text-muted-foreground",
        )}
        aria-hidden
      />

      <div className="min-w-0 flex-1">
        {entry.kind === "directory" ? (
          <button
            type="button"
            onClick={() => onOpen(entry.path)}
            className={cn(
              "max-w-full truncate rounded text-left text-sm font-medium hover:underline focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/50",
              !deletable && "text-muted-foreground",
            )}
          >
            {entry.name}
          </button>
        ) : (
          <span className={cn("block truncate text-sm", !deletable && "text-muted-foreground")}>
            {entry.name}
          </span>
        )}

        <div className="mt-1 flex items-center gap-2">
          <div className="h-1 w-full max-w-40 overflow-hidden rounded-full bg-muted">
            {entry.measured && (
              <div
                className={cn(
                  "h-full rounded-full transition-[width] duration-500",
                  entry.protection.kind === "refused" ? "bg-muted-foreground/50" : "bg-primary",
                )}
                style={{ width: `${Math.max(entry.share * 100, 1)}%` }}
              />
            )}
          </div>
          <span className="w-8 shrink-0 text-xs text-muted-foreground">
            {entry.measured ? `${Math.round(entry.share * 100)}%` : ""}
          </span>
        </div>
      </div>

      <ProtectionTag protection={entry.protection} isSymlink={entry.isSymlink} />

      <div className="hidden w-28 shrink-0 text-right text-xs text-muted-foreground sm:block">
        {formatAge(entry.lastAccessed ?? entry.lastModified)}
      </div>

      <div className="w-24 shrink-0 text-right">
        {entry.measured ? (
          <>
            <div className="text-sm font-medium tabular-nums">{formatBytes(entry.size)}</div>
            {entry.kind === "directory" && (
              <div className="text-xs text-muted-foreground tabular-nums">
                {plural(entry.fileCount, "file")}
              </div>
            )}
          </>
        ) : (
          // Showing "0 B" for a folder that simply has not been walked yet
          // would be a lie the user acts on.
          <div className="flex items-center justify-end gap-1.5 text-xs text-muted-foreground">
            <Loader2 className="size-3 animate-spin" aria-hidden />
            measuring
          </div>
        )}
      </div>

      <Button
        variant="ghost"
        size="icon-sm"
        className="opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100"
        onClick={() => void api.revealPath(entry.path).catch(() => {})}
        aria-label={`Show ${entry.name} in the file manager`}
      >
        <ExternalLink />
      </Button>
    </div>
  );
});

/**
 * The badge shown beside personal-data rows.
 *
 * Refused rows already say so via the lock in the checkbox slot, so repeating
 * it here would be noise; this only marks the middle tier, which is the one a
 * user actually has a decision to make about.
 */
function ProtectionTag({
  protection,
  isSymlink,
}: {
  protection: Protection;
  isSymlink: boolean;
}) {
  if (isSymlink) {
    return (
      <Tooltip>
        <TooltipTrigger
          render={<span tabIndex={0} className="shrink-0 cursor-help text-muted-foreground" />}
        >
          <Link2 className="size-4" aria-label="Shortcut" />
        </TooltipTrigger>
        <TooltipContent>
          A shortcut to somewhere else. SpaceKeeper never follows or removes these.
        </TooltipContent>
      </Tooltip>
    );
  }

  if (protection.kind !== "caution") return <span className="w-4 shrink-0" />;

  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <span
            tabIndex={0}
            className="inline-flex shrink-0 cursor-help items-center gap-1 rounded-full bg-warn/10 px-2 py-0.5 text-xs font-medium text-warn"
          />
        }
      >
        <AlertTriangle className="size-3" aria-hidden />
        Yours
      </TooltipTrigger>
      <TooltipContent>
        This is {protection.reason}. Nothing recreates it, so SpaceKeeper never cleans it
        automatically — but you can remove it here.
      </TooltipContent>
    </Tooltip>
  );
}

/** One-line explanation of what the icons in the list mean. */
function ExplorerLegend() {
  return (
    <div className="flex flex-wrap items-center gap-x-5 gap-y-1.5 px-1 text-xs text-muted-foreground">
      <span>Click a folder name to look inside. Backspace goes up.</span>
      <span className="flex items-center gap-1.5">
        <Lock className="size-3" aria-hidden />
        never removable
      </span>
      <span className="flex items-center gap-1.5">
        <span className="inline-flex items-center gap-1 rounded-full bg-warn/10 px-1.5 py-0.5 font-medium text-warn">
          <AlertTriangle className="size-2.5" aria-hidden />
          Yours
        </span>
        your own files — never cleaned automatically
      </span>
    </div>
  );
}

/**
 * Confirmation for explorer deletions.
 *
 * Personal data needs a second, separate acknowledgement — the checkbox is not
 * pre-ticked and the button stays disabled until it is. Nothing here can remove
 * a `refused` item; those cannot even be selected.
 */
function DeleteSelectedDialog({
  entries,
  onDone,
}: {
  entries: BrowseEntry[];
  onDone: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [acknowledged, setAcknowledged] = useState(false);
  const [permanent, setPermanent] = useState(false);
  const [done, setDone] = useState<CleanupRecord | null>(null);

  const paths = entries.map((entry) => entry.path);
  const personal = entries.filter((entry) => entry.protection.kind === "caution");
  const blocked = personal.length > 0 && !acknowledged;

  // Ask the backend what it would actually do, rather than adding up sizes
  // here. Anything it intends to skip — something already gone, something
  // protected that slipped through — is then visible *before* the user
  // commits, and the figure shown is the figure the cleanup will deliver.
  const preview = useQuery({
    queryKey: ["explorerPreview", paths.join(","), permanent],
    queryFn: () => api.deleteBrowsed(paths, permanent ? "permanent" : "trash", true),
    enabled: open && paths.length > 0 && !done,
  });

  const skipped = preview.data?.items.filter((item) => item.status === "skipped") ?? [];
  const total = preview.data?.freed ?? entries.reduce((sum, entry) => sum + entry.size, 0);

  const remove = useMutation({
    mutationFn: () => api.deleteBrowsed(paths, permanent ? "permanent" : "trash"),
    onSuccess: (record) => {
      setDone(record);
      onDone();
    },
  });

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) {
          setDone(null);
          setAcknowledged(false);
          setPermanent(false);
          remove.reset();
        }
      }}
    >
      <Button disabled={entries.length === 0} onClick={() => setOpen(true)}>
        <Trash2 />
        Delete{entries.length > 0 ? ` ${formatBytes(total)}` : ""}
      </Button>

      <DialogContent className="sm:max-w-lg">
        {done ? (
          <>
            <DialogHeader>
              <DialogTitle>Done</DialogTitle>
              <DialogDescription>
                {done.mode === "trash"
                  ? `Moved ${plural(done.removed, "item")} to the Trash, freeing ${formatBytes(done.freed)}.`
                  : `Permanently removed ${plural(done.removed, "item")}, freeing ${formatBytes(done.freed)}.`}
                {done.skipped > 0 ? ` ${plural(done.skipped, "item")} was skipped.` : ""}
              </DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <DialogClose render={<Button>Close</Button>} />
            </DialogFooter>
          </>
        ) : (
          <>
            <DialogHeader>
              <DialogTitle>Delete {plural(entries.length, "item")}?</DialogTitle>
              <DialogDescription>
                {preview.isLoading
                  ? "Checking what can be removed…"
                  : preview.data
                    ? `${plural(preview.data.removed, "item")} will be removed, freeing ${formatBytes(preview.data.freed)}.`
                    : `This will free about ${formatBytes(total)}.`}
              </DialogDescription>
            </DialogHeader>

            <div className="space-y-4 px-6">
              <ul className="max-h-40 space-y-1 overflow-y-auto rounded-lg bg-muted p-3 text-sm">
                {entries.slice(0, 8).map((entry) => (
                  <li key={entry.path} className="flex items-baseline justify-between gap-3">
                    <span className="truncate">{entry.name}</span>
                    <span className="shrink-0 tabular-nums text-muted-foreground">
                      {formatBytes(entry.size)}
                    </span>
                  </li>
                ))}
                {entries.length > 8 && (
                  <li className="text-muted-foreground">and {entries.length - 8} more…</li>
                )}
              </ul>

              {personal.length > 0 && (
                <div className="space-y-2 rounded-lg bg-warn/10 p-3">
                  <p className="flex items-start gap-2 text-sm text-warn">
                    <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden />
                    {plural(personal.length, "item")} here{" "}
                    {personal.length === 1 ? "is" : "are"} your own files —{" "}
                    {personal[0]?.protection.kind === "caution"
                      ? personal[0].protection.reason
                      : "personal data"}
                    . Nothing recreates these.
                  </p>
                  <div className="flex items-start gap-2">
                    <Checkbox
                      id="ack"
                      checked={acknowledged}
                      onCheckedChange={setAcknowledged}
                      className="mt-0.5"
                    />
                    <Label htmlFor="ack" className="text-sm">
                      I understand these are my own files and want to remove them
                    </Label>
                  </div>
                </div>
              )}

              <div className="flex items-start gap-2">
                <Checkbox
                  id="explorer-permanent"
                  checked={permanent}
                  onCheckedChange={setPermanent}
                  className="mt-0.5"
                />
                <div>
                  <Label htmlFor="explorer-permanent" className="font-medium">
                    Delete permanently instead of using the Trash
                  </Label>
                  <p className="text-sm text-muted-foreground">
                    {permanent
                      ? "This cannot be undone."
                      : "Recommended. Items go to the Trash and can be put back."}
                  </p>
                </div>
              </div>

              {skipped.length > 0 && (
                <div className="rounded-lg bg-muted p-3 text-sm">
                  <p className="mb-1 font-medium">
                    {plural(skipped.length, "item")} will be skipped
                  </p>
                  <ul className="space-y-0.5 text-muted-foreground">
                    {skipped.slice(0, 4).map((item) => (
                      <li key={item.id} className="truncate">
                        {item.path.split("/").pop()} — {item.message}
                      </li>
                    ))}
                  </ul>
                </div>
              )}

              {remove.isError && (
                <p className="rounded-lg bg-destructive/10 p-3 text-sm text-destructive">
                  {(remove.error as { message?: string })?.message ?? "The deletion failed."}
                </p>
              )}
            </div>

            <DialogFooter>
              <DialogClose render={<Button variant="outline">Cancel</Button>} />
              <Button
                variant={permanent ? "destructive" : "default"}
                disabled={
                  blocked ||
                  remove.isPending ||
                  preview.isLoading ||
                  (preview.data?.removed ?? 0) === 0
                }
                onClick={() => remove.mutate()}
              >
                {remove.isPending ? <Loader2 className="animate-spin" /> : <Trash2 />}
                {permanent ? "Delete permanently" : "Move to Trash"}
              </Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
