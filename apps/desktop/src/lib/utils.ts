import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

import type { Category, RiskLevel } from "./types"

/** Merge Tailwind classes, letting later classes win conflicts. */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}


/**
 * Format a byte count the way the OS does — SI units, so our numbers agree
 * with Finder and Explorer. Mirrors `Bytes::human` in the Rust core.
 */
export function formatBytes(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  if (unit === 0) return `${Math.round(value)} B`;
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

/** "3 days ago", "never opened" — for the last-accessed column. */
export function formatAge(timestamp: string | null): string {
  if (!timestamp) return "unknown";
  const then = new Date(timestamp).getTime();
  if (Number.isNaN(then)) return "unknown";

  const days = Math.floor((Date.now() - then) / 86_400_000);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 30) return `${days} days ago`;
  if (days < 365) return `${Math.round(days / 30)} months ago`;
  const years = (days / 365).toFixed(days < 730 ? 0 : 1);
  return `${years} ${days < 730 ? "year" : "years"} ago`;
}

export function formatDateTime(timestamp: string): string {
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return "unknown";
  return date.toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`;
}

/** Shorten `/Users/me/…` to `~/…` for display. */
export function shortenPath(path: string, maxLength = 64): string {
  if (path.length <= maxLength) return path;
  const parts = path.split(/[/\\]/);
  if (parts.length <= 3) return `…${path.slice(-maxLength)}`;
  return `${parts[0]}/${parts[1]}/…/${parts.slice(-2).join("/")}`;
}

export const RISK_LABEL: Record<RiskLevel, string> = {
  safe: "Safe",
  review: "Review",
  dangerous: "Protected",
};

/**
 * What each risk level actually means, in the user's terms rather than ours.
 * Shown in tooltips so the badge is never just a colour.
 */
export const RISK_MEANING: Record<RiskLevel, string> = {
  safe: "Recreated automatically by the app that made it. Removing it is not noticeable.",
  review: "Not recreated automatically. Look at it before deciding.",
  dangerous: "SpaceKeeper will not delete this. It is shown so you know what is using space.",
};

export const CATEGORY_LABEL: Record<Category, string> = {
  cache: "Caches",
  logs: "Logs",
  temp: "Temporary files",
  trash: "Trash",
  downloads: "Downloads",
  desktop: "Desktop",
  duplicate: "Duplicates",
  largeFile: "Large files",
  emptyFolder: "Empty folders",
  packageCache: "Package caches",
  buildArtifact: "Build artifacts",
  container: "Containers",
  browserCache: "Browser caches",
  aiModel: "AI models",
  deviceImage: "Simulators & emulators",
  other: "Other",
};

/** Pluralise a count without a dependency. */
export function plural(count: number, singular: string, pluralForm?: string): string {
  return `${count.toLocaleString()} ${count === 1 ? singular : (pluralForm ?? `${singular}s`)}`;
}
