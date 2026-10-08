import { cn, formatBytes } from "@/lib/utils";
import type { DiskInfo } from "@/lib/types";

/**
 * The disk usage ring.
 *
 * The reclaimable slice is drawn *inside* the used arc rather than beside it,
 * because that is what it is: space that is currently used and could stop
 * being. Showing it as a separate segment would imply it was already free.
 */
export function DiskGauge({
  disk,
  reclaimable,
  className,
}: {
  disk: DiskInfo;
  reclaimable: number;
  className?: string;
}) {
  const used = Math.max(disk.total - disk.available, 0);
  const usedFraction = disk.total > 0 ? used / disk.total : 0;
  const reclaimableFraction = disk.total > 0 ? Math.min(reclaimable, used) / disk.total : 0;

  const radius = 78;
  const stroke = 14;
  const circumference = 2 * Math.PI * radius;
  const critical = usedFraction >= 0.9;

  return (
    <div className={cn("flex items-center gap-6", className)}>
      <div className="relative shrink-0">
        <svg
          width="184"
          height="184"
          viewBox="0 0 184 184"
          role="img"
          aria-label={`${Math.round(usedFraction * 100)} percent of ${formatBytes(disk.total)} used, ${formatBytes(reclaimable)} reclaimable`}
        >
          <g transform="rotate(-90 92 92)">
            <circle
              cx="92"
              cy="92"
              r={radius}
              fill="none"
              strokeWidth={stroke}
              className="stroke-muted"
            />
            <circle
              cx="92"
              cy="92"
              r={radius}
              fill="none"
              strokeWidth={stroke}
              strokeLinecap="round"
              strokeDasharray={`${usedFraction * circumference} ${circumference}`}
              className={cn(
                "transition-[stroke-dasharray] duration-700 ease-out",
                // `primary`, not `accent`: in this token set `accent` is the
                // subtle hover background, which is almost invisible on the
                // track it sits on.
                critical ? "stroke-destructive" : "stroke-primary",
              )}
            />
            {reclaimableFraction > 0.002 && (
              <circle
                cx="92"
                cy="92"
                r={radius}
                fill="none"
                strokeWidth={stroke}
                strokeLinecap="round"
                strokeDasharray={`${reclaimableFraction * circumference} ${circumference}`}
                strokeDashoffset={-(usedFraction - reclaimableFraction) * circumference}
                className="stroke-safe transition-[stroke-dasharray] duration-700 ease-out"
              />
            )}
          </g>
        </svg>
        <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center">
          <span className="text-3xl font-semibold tabular-nums text-foreground">
            {Math.round(usedFraction * 100)}%
          </span>
          <span className="text-xs text-muted-foreground">used</span>
        </div>
      </div>

      <dl className="min-w-0 space-y-3 text-sm">
        <Row label="Capacity" value={formatBytes(disk.total)} />
        <Row label="Free now" value={formatBytes(disk.available)} />
        <Row
          label="Reclaimable"
          value={reclaimable > 0 ? formatBytes(reclaimable) : "—"}
          swatch="bg-safe"
          emphasis
        />
        <div className="truncate pt-1 text-xs text-muted-foreground">
          {disk.name} · {disk.fileSystem} · {disk.mountPoint}
        </div>
      </dl>
    </div>
  );
}

function Row({
  label,
  value,
  swatch,
  emphasis,
}: {
  label: string;
  value: string;
  swatch?: string;
  emphasis?: boolean;
}) {
  return (
    <div className="flex items-center gap-2">
      {swatch ? <span className={cn("size-2 rounded-full", swatch)} aria-hidden /> : null}
      <dt className="w-24 shrink-0 text-muted-foreground">{label}</dt>
      <dd className={cn("tabular-nums", emphasis ? "font-semibold text-safe" : "text-foreground")}>
        {value}
      </dd>
    </div>
  );
}
