import { useQuery } from "@tanstack/react-query";
import { TrendingDown, TrendingUp } from "lucide-react";
import { useMemo } from "react";
import { Area, AreaChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";

import { Skeleton } from "@/components/ui/skeleton";
import * as api from "@/lib/api";
import { cn, formatBytes } from "@/lib/utils";

/**
 * Free space over time.
 *
 * Every scan writes a disk snapshot, so this is the one view that answers "is
 * this getting better or worse?" — which is the question a cleanup tool should
 * be judged on. Two or fewer points is not a trend, so the chart stays hidden
 * until there is something real to show rather than drawing a single dot.
 */
export function StorageTrend() {
  const trend = useQuery({ queryKey: ["diskTrend"], queryFn: () => api.diskTrend(60) });

  const points = useMemo(
    () =>
      (trend.data ?? []).map((snapshot) => ({
        at: new Date(snapshot.takenAt).getTime(),
        free: snapshot.available,
      })),
    [trend.data],
  );

  if (trend.isLoading) return <Skeleton className="h-40 w-full" />;
  if (points.length < 3) {
    return (
      <p className="py-8 text-center text-sm text-muted-foreground">
        Free space over time appears here once you have run a few scans.
      </p>
    );
  }

  const first = points[0]?.free ?? 0;
  const last = points[points.length - 1]?.free ?? 0;
  const change = last - first;
  const gaining = change >= 0;

  // A y-axis anchored at zero flattens a 4 GB change on a 500 GB disk into a
  // straight line. Padding around the actual range shows the shape instead.
  const values = points.map((point) => point.free);
  const min = Math.min(...values);
  const max = Math.max(...values);
  const padding = Math.max((max - min) * 0.2, 1);

  return (
    <div className="space-y-3">
      <div className="flex items-baseline gap-2 text-sm">
        {gaining ? (
          <TrendingUp className="size-4 shrink-0 self-center text-safe" aria-hidden />
        ) : (
          <TrendingDown className="size-4 shrink-0 self-center text-warn" aria-hidden />
        )}
        <span className={cn("font-semibold tabular-nums", gaining ? "text-safe" : "text-warn")}>
          {gaining ? "+" : "−"}
          {formatBytes(Math.abs(change))}
        </span>
        <span className="text-muted-foreground">
          free space {gaining ? "gained" : "lost"} since SpaceKeeper started watching
        </span>
      </div>

      <div className="h-40 w-full">
        <ResponsiveContainer width="100%" height="100%">
          <AreaChart data={points} margin={{ top: 4, right: 4, bottom: 0, left: 0 }}>
            <defs>
              <linearGradient id="freeSpace" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stopColor="var(--color-primary)" stopOpacity={0.35} />
                <stop offset="100%" stopColor="var(--color-primary)" stopOpacity={0} />
              </linearGradient>
            </defs>
            <XAxis
              dataKey="at"
              type="number"
              scale="time"
              domain={["dataMin", "dataMax"]}
              tickFormatter={(value) =>
                new Date(Number(value)).toLocaleDateString(undefined, {
                  month: "short",
                  day: "numeric",
                })
              }
              tick={{ fontSize: 11, fill: "var(--color-muted-foreground)" }}
              axisLine={false}
              tickLine={false}
              minTickGap={40}
            />
            <YAxis
              domain={[Math.max(min - padding, 0), max + padding]}
              tickFormatter={(value) => formatBytes(Number(value))}
              tick={{ fontSize: 11, fill: "var(--color-muted-foreground)" }}
              axisLine={false}
              tickLine={false}
              width={64}
            />
            <Tooltip
              contentStyle={{
                background: "var(--color-popover)",
                border: "1px solid var(--color-border)",
                borderRadius: "var(--radius-lg)",
                fontSize: 12,
                color: "var(--color-popover-foreground)",
              }}
              labelFormatter={(value) => new Date(Number(value)).toLocaleString()}
              formatter={(value) => [formatBytes(Number(value)), "Free"] as [string, string]}
            />
            <Area
              type="monotone"
              dataKey="free"
              stroke="var(--color-primary)"
              strokeWidth={2}
              fill="url(#freeSpace)"
              // The data is sampled at scan time, not continuously, so an
              // animation implying smooth motion between points would be
              // suggesting precision that is not there.
              isAnimationActive={false}
            />
          </AreaChart>
        </ResponsiveContainer>
      </div>
    </div>
  );
}
