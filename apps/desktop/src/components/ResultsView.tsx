import { FolderSearch, Folder, File as FileIcon, ExternalLink } from "lucide-react";
import { memo, useMemo, useState } from "react";

import { CleanupDialog } from "@/components/CleanupDialog";
import { VirtualList } from "@/components/VirtualList";
import { RiskBadge } from "@/components/RiskBadge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import * as api from "@/lib/api";
import { summarizeSelection, useAppStore } from "@/lib/store";
import type { RiskLevel, ScanReport, ScanResult } from "@/lib/types";
import { cn, formatAge, formatBytes, plural } from "@/lib/utils";

type Filter = "all" | RiskLevel;

/**
 * The results list.
 *
 * Ordered by the backend's recommendation score, not by size — the whole point
 * of the score is that a stale 2 GB cache is a better suggestion than an
 * active 20 GB one. The list preserves that order and does not re-sort.
 */
export function ResultsView({ report }: { report: ScanReport | undefined }) {
  const [filter, setFilter] = useState<Filter>("all");
  // Zustand actions are stable identities, which is what lets the memoised
  // rows below actually skip re-rendering.
  const { selection, toggle, setSelection, clearSelection } = useAppStore();

  const visible = useMemo(() => {
    if (!report) return [];
    return filter === "all" ? report.results : report.results.filter((r) => r.risk === filter);
  }, [report, filter]);

  const selectable = visible.filter((result) => result.deletable);
  const selectedHere = selectable.filter((result) => selection.has(result.id));
  const allSelected = selectable.length > 0 && selectedHere.length === selectable.length;

  const summary = summarizeSelection(report, selection);

  if (!report) {
    return (
      <Empty>
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <FolderSearch />
          </EmptyMedia>
          <EmptyTitle>Nothing scanned yet</EmptyTitle>
          <EmptyDescription>
            Run a scan to see what is using space. Nothing is deleted without you selecting it
            first.
          </EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-wrap items-center gap-3 border-b border-border pb-3">
        <div className="flex items-center gap-1 rounded-lg bg-muted p-1">
          {(["all", "safe", "review", "dangerous"] as const).map((option) => (
            <button
              key={option}
              type="button"
              onClick={() => setFilter(option)}
              aria-pressed={filter === option}
              className={cn(
                "rounded-md px-3 py-1 text-sm font-medium transition-colors",
                "focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/50",
                filter === option
                  ? "bg-card text-foreground shadow-sm"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              {option === "all"
                ? "Everything"
                : option === "safe"
                  ? "Safe"
                  : option === "review"
                    ? "Needs review"
                    : "Protected"}
            </button>
          ))}
        </div>

        <span className="text-sm text-muted-foreground">
          {plural(visible.length, "item")}
          {selectable.length > 0 && (
            <>
              {" · "}
              <button
                type="button"
                className="underline underline-offset-2 hover:text-foreground"
                onClick={() =>
                  allSelected
                    ? clearSelection()
                    : setSelection([
                        ...selection,
                        ...selectable.map((result) => result.id),
                      ])
                }
              >
                {allSelected ? "Deselect all" : "Select all shown"}
              </button>
            </>
          )}
        </span>

        <div className="ml-auto flex items-center gap-3">
          {summary.count > 0 && (
            <span className="text-sm tabular-nums text-muted-foreground">
              {plural(summary.count, "item")} · {formatBytes(summary.size)}
            </span>
          )}
          <CleanupDialog report={report} />
        </div>
      </div>

      {visible.length === 0 ? (
        <p className="py-14 text-center text-sm text-muted-foreground">
          Nothing in this category.
        </p>
      ) : (
        <VirtualList
          items={visible}
          estimateHeight={116}
          getKey={(result) => result.id}
          renderRow={(result) => (
            <ResultRow
              result={result}
              checked={selection.has(result.id)}
              onToggle={toggle}
            />
          )}
        />
      )}
    </div>
  );
}

/**
 * One result.
 *
 * Memoised and given a path-taking callback, so ticking one checkbox in a
 * thousand-row list re-renders one row rather than all of them.
 */
const ResultRow = memo(function ResultRow({
  result,
  checked,
  onToggle,
}: {
  result: ScanResult;
  checked: boolean;
  onToggle: (id: string) => void;
}) {
  const Icon = result.kind === "directory" ? Folder : FileIcon;

  return (
    <div className="flex items-start gap-3 border-b border-border px-1 py-3 hover:bg-muted/40">
      <div className="pt-1">
        <Checkbox
          checked={checked}
          onCheckedChange={() => onToggle(result.id)}
          disabled={!result.deletable}
          aria-label={
            result.deletable
              ? `Select ${result.title}`
              : `${result.title} cannot be removed by SpaceKeeper`
          }
        />
      </div>

      <Icon className="mt-1 size-4 shrink-0 text-muted-foreground" aria-hidden />

      <div className="min-w-0 flex-1 space-y-1">
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <span className="truncate font-medium">{result.title}</span>
          <RiskBadge risk={result.risk} />
          {!result.recoverable && result.deletable && (
            <Tooltip>
              <TooltipTrigger
                render={
                  <span
                    tabIndex={0}
                    className="cursor-help rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground"
                  />
                }
              >
                not undoable
              </TooltipTrigger>
              <TooltipContent>
                This cannot be restored from the Trash once removed.
              </TooltipContent>
            </Tooltip>
          )}
        </div>

        <p className="text-sm leading-relaxed text-muted-foreground">{result.description}</p>

        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
          <code className="selectable truncate font-mono">{result.path}</code>
          <span>·</span>
          <span>last used {formatAge(result.lastAccessed ?? result.lastModified)}</span>
          {result.fileCount > 1 && (
            <>
              <span>·</span>
              <span>{plural(result.fileCount, "file")}</span>
            </>
          )}
          <Button
            variant="ghost"
            size="xs"
            className="ml-auto"
            onClick={() => void api.revealPath(result.path).catch(() => {})}
          >
            <ExternalLink />
            Show
          </Button>
        </div>
      </div>

      <div className="shrink-0 pt-0.5 text-right">
        <div className="font-semibold tabular-nums">{formatBytes(result.size)}</div>
        <div className="text-xs text-muted-foreground tabular-nums">
          score {Math.round(result.score)}
        </div>
      </div>
    </div>
  );
});
