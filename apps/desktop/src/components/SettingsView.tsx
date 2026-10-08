import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Lock } from "lucide-react";

import { RiskBadge } from "@/components/RiskBadge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import * as api from "@/lib/api";
import type { Family, ScanOptions } from "@/lib/types";
import { formatBytes } from "@/lib/utils";

const FAMILY_LABEL: Record<Family, string> = {
  general: "General",
  developer: "Developer",
  browser: "Browsers",
  ai: "AI models",
};

/**
 * Settings.
 *
 * The protected-paths list is shown read-only and in full. A cleanup tool
 * asking for this much trust should be able to say exactly what it will never
 * touch, without the user having to read the source.
 */
export function SettingsView() {
  const queryClient = useQueryClient();
  const scanners = useQuery({ queryKey: ["scanners"], queryFn: api.listScanners });
  const settings = useQuery({ queryKey: ["settings"], queryFn: api.getSettings });
  const protectedList = useQuery({ queryKey: ["protectedPaths"], queryFn: api.protectedPaths });

  const save = useMutation({
    mutationFn: (options: ScanOptions) => api.saveSettings(options),
    onSuccess: (saved) => queryClient.setQueryData(["settings"], saved),
  });

  const update = (patch: Partial<ScanOptions>) => {
    if (!settings.data) return;
    save.mutate({ ...settings.data, ...patch });
  };

  const toggleScanner = (id: string, enabled: boolean) => {
    if (!settings.data) return;
    const disabled = new Set(settings.data.disabledScanners);
    if (enabled) disabled.delete(id);
    else disabled.add(id);
    update({ disabledScanners: [...disabled] });
  };

  const grouped = (scanners.data ?? []).reduce<Record<string, typeof scanners.data>>(
    (acc, scanner) => {
      (acc[scanner.family] ??= []).push(scanner);
      return acc;
    },
    {} as Record<string, NonNullable<typeof scanners.data>>,
  );

  return (
    <div className="space-y-5">
      <Card>
        <CardHeader>
          <CardTitle>Scan settings</CardTitle>
          <CardDescription>
            These change what a scan reports. They never change what SpaceKeeper is willing to
            delete.
          </CardDescription>
        </CardHeader>
        <CardContent className="grid gap-5 sm:grid-cols-2">
          {settings.isLoading || !settings.data ? (
            <Skeleton className="h-24 w-full sm:col-span-2" />
          ) : (
            <>
              <NumberSetting
                id="stale"
                label="Treat files as unused after"
                suffix="days"
                value={settings.data.staleAfterDays}
                onCommit={(value) => update({ staleAfterDays: value })}
                hint="Downloads older than this are suggested, and node_modules for projects you have not touched."
              />
              <NumberSetting
                id="min-size"
                label="Ignore results smaller than"
                suffix="MB"
                value={Math.round(settings.data.minResultSize / 1_000_000)}
                onCommit={(value) => update({ minResultSize: value * 1_000_000 })}
                hint="Keeps a hundred tiny caches out of the list."
              />
              <NumberSetting
                id="large"
                label="A file counts as large at"
                suffix="MB"
                value={Math.round(settings.data.largeFileThreshold / 1_000_000)}
                onCommit={(value) => update({ largeFileThreshold: value * 1_000_000 })}
                hint="Used by the large-files scanner."
              />
              <NumberSetting
                id="depth"
                label="Maximum folder depth"
                suffix="levels"
                value={settings.data.maxDepth}
                onCommit={(value) => update({ maxDepth: value })}
                hint="Lower is faster; higher finds more."
              />
            </>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Scanners</CardTitle>
          <CardDescription>
            Turn off anything you do not want SpaceKeeper to look at. Each one says what it finds.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          {scanners.isLoading ? (
            <Skeleton className="h-40 w-full" />
          ) : (
            Object.entries(grouped).map(([family, list]) => (
              <section key={family} className="space-y-3">
                <h3 className="text-sm font-semibold text-muted-foreground">
                  {FAMILY_LABEL[family as Family] ?? family}
                </h3>
                <ul className="space-y-3">
                  {(list ?? []).map((scanner) => {
                    const enabled = !settings.data?.disabledScanners.includes(scanner.id);
                    return (
                      <li key={scanner.id} className="flex items-start gap-4">
                        <Switch
                          checked={enabled}
                          onCheckedChange={(checked) => toggleScanner(scanner.id, checked)}
                          aria-label={`${scanner.name} scanner`}
                          className="mt-0.5"
                        />
                        <div className="min-w-0 flex-1 space-y-1">
                          <div className="flex flex-wrap items-center gap-2">
                            <span className="font-medium">{scanner.name}</span>
                            <RiskBadge risk={scanner.defaultRisk} />
                            {scanner.requiresTool && (
                              <span className="rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground">
                                needs {scanner.requiresTool}
                              </span>
                            )}
                          </div>
                          <p className="text-sm text-muted-foreground">{scanner.explanation}</p>
                        </div>
                      </li>
                    );
                  })}
                </ul>
              </section>
            ))
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Lock className="size-4" aria-hidden />
            Never touched
          </CardTitle>
          <CardDescription>
            Enforced in Rust at the moment of deletion, not just when results are shown. Locked
            items can never be removed by anything in SpaceKeeper. Personal items are never cleaned
            automatically, but you can remove them yourself from the Explore tab.
          </CardDescription>
        </CardHeader>
        <CardContent>
          {protectedList.isLoading ? (
            <Skeleton className="h-32 w-full" />
          ) : (
            <ul className="selectable space-y-1.5 text-xs">
              {protectedList.data?.map((entry) => (
                <li key={entry.path} className="flex items-baseline gap-2">
                  {entry.refused ? (
                    <Lock
                      className="size-3 shrink-0 translate-y-0.5 text-safe"
                      aria-label="Never removable"
                    />
                  ) : (
                    <AlertTriangle
                      className="size-3 shrink-0 translate-y-0.5 text-warn"
                      aria-label="Removable only from the explorer"
                    />
                  )}
                  <code className="font-mono text-muted-foreground">{entry.path}</code>
                  <span className="text-muted-foreground/80">— {entry.reason}</span>
                </li>
              ))}
            </ul>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Privacy</CardTitle>
        </CardHeader>
        <CardContent className="space-y-2 text-sm text-muted-foreground">
          <p>
            SpaceKeeper has no network code. Scans, history and settings stay in a local SQLite
            file, and no file contents, paths or statistics are ever transmitted.
          </p>
          <p>
            Duplicate detection hashes file contents locally with BLAKE3; the hashes never leave
            this machine and are not stored after the scan.
          </p>
        </CardContent>
      </Card>
    </div>
  );
}

function NumberSetting({
  id,
  label,
  suffix,
  value,
  onCommit,
  hint,
}: {
  id: string;
  label: string;
  suffix: string;
  value: number;
  onCommit: (value: number) => void;
  hint: string;
}) {
  return (
    <div className="space-y-1.5">
      <Label htmlFor={id}>{label}</Label>
      <div className="flex items-center gap-2">
        <Input
          id={id}
          type="number"
          min={0}
          defaultValue={value}
          className="w-28"
          // Committed on blur rather than per keystroke: every change is a
          // database write, and a half-typed "1" is not a setting.
          onBlur={(event) => {
            const next = Number(event.currentTarget.value);
            if (Number.isFinite(next) && next >= 0 && next !== value) onCommit(next);
          }}
        />
        <span className="text-sm text-muted-foreground">{suffix}</span>
      </div>
      <p className="text-xs text-muted-foreground">{hint}</p>
      {suffix === "MB" && <p className="sr-only">Currently {formatBytes(value * 1_000_000)}</p>}
    </div>
  );
}
