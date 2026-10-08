/**
 * Typed wrappers around the Tauri commands.
 *
 * Every backend call goes through this file, so there is one place that knows
 * command names and one place to look when the contract changes.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  CleanupMode,
  CleanupRecord,
  DirectoryListing,
  DiskInfo,
  DiskSnapshot,
  Explanation,
  ScanHistoryEntry,
  ScanOptions,
  ScanProgress,
  ScanReport,
  ScannerMetadata,
  ScannerStats,
  MeasuredEvent,
  ProtectedEntry,
  SortBy,
} from "./types";

export const PROGRESS_EVENT = "scan://progress";
export const MEASURED_EVENT = "browse://measured";

/** Whether the app is running inside Tauri, rather than a plain browser tab. */
export const isTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// ------------------------------------------------------------------ scanning

export const listScanners = () => invoke<ScannerMetadata[]>("list_scanners");

/** Run a scan. Pass scanner ids to run a subset. */
export const startScan = (scanners?: string[]) =>
  invoke<ScanReport>("start_scan", { scanners: scanners ?? null });

export const cancelScan = () => invoke<void>("cancel_scan");
export const pauseScan = () => invoke<void>("pause_scan");
export const resumeScan = () => invoke<void>("resume_scan");
export const scanStatus = () => invoke<{ paused: boolean; cancelled: boolean }>("scan_status");

/** Subscribe to progress events for the duration of a scan. */
export const onScanProgress = (handler: (progress: ScanProgress) => void): Promise<UnlistenFn> =>
  listen<ScanProgress>(PROGRESS_EVENT, (event) => handler(event.payload));

// ------------------------------------------------------------------- cleanup

/**
 * Preview a cleanup. Nothing is touched; the returned record describes what
 * would happen, and is what the confirmation dialog shows.
 */
export const previewCleanup = (itemIds: string[], mode: CleanupMode = "trash") =>
  invoke<CleanupRecord>("preview_cleanup", { command: { itemIds, mode } });

export const runCleanup = (itemIds: string[], mode: CleanupMode = "trash") =>
  invoke<CleanupRecord>("run_cleanup", { command: { itemIds, mode } });

export const undoCleanup = (cleanupId: string) => invoke<string[]>("undo_cleanup", { cleanupId });

export const undoSupported = () => invoke<boolean>("undo_supported");

// ------------------------------------------------------------------ explorer

/** List a folder with every child measured. Omit `path` to start at home. */
export const browseDirectory = (path?: string, sort?: SortBy) =>
  invoke<DirectoryListing>("browse_directory", { path: path ?? null, sort: sort ?? null });

/**
 * Remove items picked in the explorer.
 *
 * Only paths the backend listed in the folder currently on screen are accepted,
 * which keeps the frontend from naming an arbitrary path even though this call
 * is path-addressed.
 */
export const deleteBrowsed = (paths: string[], mode: CleanupMode = "trash", dryRun = false) =>
  invoke<CleanupRecord>("delete_browsed", { paths, mode, dryRun });

/** The generation token of the listing the backend last produced. */
export const browseToken = () => invoke<number>("browse_token");

/**
 * Subscribe to streamed directory sizes.
 *
 * The listing returns before its folders have been walked; each size arrives
 * here as it lands, tagged with the listing it belongs to.
 */
export const onMeasured = (handler: (event: MeasuredEvent) => void): Promise<UnlistenFn> =>
  listen<MeasuredEvent>(MEASURED_EVENT, (event) => handler(event.payload));

// --------------------------------------------------------------- information

export const listDisks = () => invoke<DiskInfo[]>("list_disks");
export const explainDisk = () => invoke<Explanation | null>("explain_disk");
export const scanHistory = (limit?: number) =>
  invoke<ScanHistoryEntry[]>("scan_history", { limit: limit ?? null });
export const cleanupHistory = (limit?: number) =>
  invoke<CleanupRecord[]>("cleanup_history", { limit: limit ?? null });
export const totalFreed = () => invoke<number>("total_freed");
export const diskTrend = (limit?: number) =>
  invoke<DiskSnapshot[]>("disk_trend", { limit: limit ?? null });
export const scannerStats = () => invoke<ScannerStats[]>("scanner_stats");

// ------------------------------------------------------------------ settings

export const getSettings = () => invoke<ScanOptions>("get_settings");
export const saveSettings = (options: ScanOptions) =>
  invoke<ScanOptions>("save_settings", { options });
export const ignoredPaths = () => invoke<string[]>("ignored_paths");
export const addIgnoredPath = (path: string) => invoke<string[]>("add_ignored_path", { path });
export const removeIgnoredPath = (path: string) =>
  invoke<string[]>("remove_ignored_path", { path });
export const protectedPaths = () => invoke<ProtectedEntry[]>("protected_paths");
export const revealPath = (path: string) => invoke<void>("reveal_path", { path });
