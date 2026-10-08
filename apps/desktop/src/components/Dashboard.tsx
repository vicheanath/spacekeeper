import { useQuery } from "@tanstack/react-query";
import { ArrowRight, HardDrive, Info, LineChart, Sparkles, TrendingDown } from "lucide-react";

import { lazy, Suspense } from "react";

import { DiskGauge } from "@/components/DiskGauge";
import { RiskBadge } from "@/components/RiskBadge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import * as api from "@/lib/api";
import { useAppStore } from "@/lib/store";
import type { ScanReport } from "@/lib/types";
import { cn, formatBytes, plural } from "@/lib/utils";

// The charting library is the single largest dependency in the app and only
// one card needs it, so it is fetched when that card first renders rather than
// being paid for on every cold start.
const StorageTrend = lazy(() =>
  import("@/components/StorageTrend").then((module) => ({ default: module.StorageTrend })),
);

/**
 * The dashboard answers one question before anything else: *why* is the disk
 * full? The headline sentence and the findings list come from the backend's
 * explanation engine, so the words a user reads are generated from the same
 * numbers the cleanup will act on.
 */
export function Dashboard({
  report,
  onShowResults,
}: {
  report: ScanReport | undefined;
  onShowResults: () => void;
}) {
  const setSelection = useAppStore((state) => state.setSelection);

  const disks = useQuery({ queryKey: ["disks"], queryFn: api.listDisks });
  const explanation = useQuery({
    queryKey: ["explanation", report?.id],
    queryFn: api.explainDisk,
    enabled: Boolean(report),
  });
  const freed = useQuery({ queryKey: ["totalFreed"], queryFn: api.totalFreed });

  const primary = disks.data?.[0];
  const reclaimable = report?.summary.safeSize ?? 0;

  const selectAllSafe = () => {
    if (!report) return;
    setSelection(
      report.results.filter((result) => result.risk === "safe" && result.deletable).map((r) => r.id),
    );
    onShowResults();
  };

  return (
    <div className="space-y-5">
      <div className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_20rem]">
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <HardDrive className="size-4" aria-hidden />
              Your disk
            </CardTitle>
          </CardHeader>
          <CardContent>
            {disks.isLoading ? (
              <Skeleton className="h-44 w-full" />
            ) : primary ? (
              <DiskGauge disk={primary} reclaimable={reclaimable} />
            ) : (
              <p className="text-sm text-muted-foreground">No volumes reported by the system.</p>
            )}
          </CardContent>
        </Card>

        <div className="space-y-5">
          <StatCard
            icon={<Sparkles className="size-4" aria-hidden />}
            label="Reclaimable safely"
            value={report ? formatBytes(report.summary.safeSize) : "—"}
            hint={
              report
                ? `${plural(report.summary.totalItems, "item")} found in total`
                : "Run a scan to find out"
            }
            tone="safe"
            action={
              report && report.summary.safeSize > 0 ? (
                <Button size="sm" variant="outline" onClick={selectAllSafe}>
                  Select all safe items
                </Button>
              ) : undefined
            }
          />
          <StatCard
            icon={<TrendingDown className="size-4" aria-hidden />}
            label="Freed with SpaceKeeper"
            value={freed.data ? formatBytes(freed.data) : "0 B"}
            hint="Since you installed it"
          />
        </div>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Why your disk is full</CardTitle>
          <CardDescription>
            Generated on this machine from your last scan. Nothing here was sent anywhere.
          </CardDescription>
        </CardHeader>
        <CardContent>
          {!report ? (
            <Empty className="border-none py-8">
              <EmptyHeader>
                <EmptyMedia variant="icon">
                  <Info />
                </EmptyMedia>
                <EmptyTitle>No scan yet</EmptyTitle>
                <EmptyDescription>
                  Run a scan and SpaceKeeper will explain, in plain language, what is using your
                  space and what is safe to remove.
                </EmptyDescription>
              </EmptyHeader>
            </Empty>
          ) : explanation.isLoading ? (
            <Skeleton className="h-32 w-full" />
          ) : explanation.data ? (
            <div className="space-y-5">
              <p className="text-lg font-medium leading-snug text-foreground">
                {explanation.data.headline}
              </p>

              <ul className="space-y-2.5">
                {explanation.data.findings.map((finding) => (
                  <li
                    key={finding.scanner}
                    className="flex flex-wrap items-baseline gap-x-3 gap-y-1 rounded-lg bg-muted/50 px-3 py-2.5"
                  >
                    <span className="font-medium text-foreground">{finding.sentence}</span>
                    <RiskBadge risk={finding.risk} />
                    <span className="w-full text-sm text-muted-foreground">{finding.reason}</span>
                  </li>
                ))}
              </ul>

              {explanation.data.nextSteps.length > 0 && (
                <div className="rounded-lg border border-border p-4">
                  <h3 className="mb-2 text-sm font-semibold">What to do next</h3>
                  <ol className="space-y-1.5 text-sm text-muted-foreground">
                    {explanation.data.nextSteps.map((step, index) => (
                      <li key={step} className="flex gap-2">
                        <span className="tabular-nums text-muted-foreground/70">{index + 1}.</span>
                        <span>{step}</span>
                      </li>
                    ))}
                  </ol>
                </div>
              )}
            </div>
          ) : null}
        </CardContent>
      </Card>

      {report && report.results.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>Start here</CardTitle>
            <CardDescription>
              The best things to remove first — ranked by size, how long since you used them, and
              how safe they are, not by size alone.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <ul className="divide-y divide-border">
              {report.results.slice(0, 5).map((result) => (
                <li key={result.id} className="flex items-center gap-3 py-2.5">
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                      <span className="truncate text-sm font-medium">{result.title}</span>
                      <RiskBadge risk={result.risk} />
                    </div>
                    <p className="truncate text-xs text-muted-foreground">{result.description}</p>
                  </div>
                  <span className="shrink-0 text-sm font-semibold tabular-nums">
                    {formatBytes(result.size)}
                  </span>
                </li>
              ))}
            </ul>
            <Button variant="ghost" size="sm" className="mt-3" onClick={onShowResults}>
              See all {report.summary.totalItems} findings
              <ArrowRight />
            </Button>
          </CardContent>
        </Card>
      )}

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <LineChart className="size-4" aria-hidden />
            Free space over time
          </CardTitle>
          <CardDescription>Recorded locally on every scan.</CardDescription>
        </CardHeader>
        <CardContent>
          <Suspense fallback={<Skeleton className="h-40 w-full" />}>
            <StorageTrend />
          </Suspense>
        </CardContent>
      </Card>

      {report && report.summary.byCategory.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle>Where the space went</CardTitle>
            <CardDescription>Grouped by what the files are, largest first.</CardDescription>
          </CardHeader>
          <CardContent>
            <CategoryBars report={report} />
          </CardContent>
        </Card>
      )}
    </div>
  );
}

function StatCard({
  icon,
  label,
  value,
  hint,
  tone,
  action,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
  hint: string;
  tone?: "safe";
  action?: React.ReactNode;
}) {
  return (
    <Card>
      <CardContent className="space-y-1 p-5">
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          {icon}
          {label}
        </div>
        <p
          className={cn(
            "text-3xl font-semibold tabular-nums",
            tone === "safe" ? "text-safe" : "text-foreground",
          )}
        >
          {value}
        </p>
        <p className="text-xs text-muted-foreground">{hint}</p>
        {action ? <div className="pt-2">{action}</div> : null}
      </CardContent>
    </Card>
  );
}

function CategoryBars({ report }: { report: ScanReport }) {
  const largest = report.summary.byCategory[0]?.size ?? 1;

  return (
    <ul className="space-y-3">
      {report.summary.byCategory.slice(0, 10).map((group) => (
        <li key={group.key} className="space-y-1.5">
          <div className="flex items-baseline justify-between gap-4 text-sm">
            <span className="truncate font-medium">{group.label}</span>
            <span className="shrink-0 tabular-nums text-muted-foreground">
              {formatBytes(group.size)} · {plural(group.items, "item")}
            </span>
          </div>
          <div
            className="h-2 overflow-hidden rounded-full bg-muted"
            role="img"
            aria-label={`${group.label}: ${formatBytes(group.size)}`}
          >
            <div
              className="h-full rounded-full bg-primary transition-[width] duration-500"
              style={{ width: `${Math.max((group.size / largest) * 100, 1.5)}%` }}
            />
          </div>
        </li>
      ))}
    </ul>
  );
}
