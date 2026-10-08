/**
 * Mirrors of the Rust types that cross the Tauri boundary.
 *
 * Kept by hand and deliberately: the surface is small, and a hand-written
 * mirror is easier to read than generated output. `npm run typecheck` fails
 * loudly the moment a field is used that the backend does not send, and the
 * contract tests in `src-tauri` assert the serialised shape.
 */

export type RiskLevel = "safe" | "review" | "dangerous";

export type Family = "general" | "developer" | "browser" | "ai";

export type Category =
  | "cache"
  | "logs"
  | "temp"
  | "trash"
  | "downloads"
  | "desktop"
  | "duplicate"
  | "largeFile"
  | "emptyFolder"
  | "packageCache"
  | "buildArtifact"
  | "container"
  | "browserCache"
  | "aiModel"
  | "deviceImage"
  | "other";

export type EntryKind = "file" | "directory";

export interface ScannerMetadata {
  id: string;
  name: string;
  description: string;
  /** Plain-language explanation shown to the user before they decide. */
  explanation: string;
  family: Family;
  defaultRisk: RiskLevel;
  platforms: string[];
  enabledByDefault: boolean;
  requiresTool: string | null;
}

export interface ScanResult {
  id: string;
  scanner: string;
  title: string;
  description: string;
  path: string;
  /** Bytes. */
  size: number;
  kind: EntryKind;
  fileCount: number;
  category: Category;
  risk: RiskLevel;
  deletable: boolean;
  recoverable: boolean;
  lastAccessed: string | null;
  lastModified: string | null;
  /** 0–100, produced by the recommendation engine. */
  score: number;
  confidence: number;
  detail?: Record<string, unknown>;
}

export interface GroupSummary {
  key: string;
  label: string;
  items: number;
  size: number;
}

export interface ResultsSummary {
  totalItems: number;
  totalSize: number;
  /** Reclaimable with no review needed. */
  safeSize: number;
  reviewSize: number;
  byCategory: GroupSummary[];
  byScanner: GroupSummary[];
}

export type ScannerStatus = "completed" | "skipped" | "failed" | "cancelled";

export interface ScannerOutcome {
  scanner: string;
  name: string;
  status: ScannerStatus;
  items: number;
  size: number;
  durationMs: number;
  message: string | null;
}

export interface ScanReport {
  id: string;
  startedAt: string;
  durationMs: number;
  cancelled: boolean;
  results: ScanResult[];
  summary: ResultsSummary;
  scanners: ScannerOutcome[];
}

export type Phase = "started" | "scanning" | "finished" | "skipped" | "failed";

export interface ScanProgress {
  scanner: string;
  phase: Phase;
  filesScanned: number;
  bytesFound: number;
  currentPath: string | null;
  scannersTotal: number;
  message: string | null;
}

export type CleanupMode = "trash" | "permanent";

export type ItemStatus = "trashed" | "deleted" | "planned" | "skipped" | "failed";

export interface ItemOutcome {
  id: string;
  path: string;
  size: number;
  status: ItemStatus;
  message: string | null;
}

export interface CleanupRecord {
  id: string;
  performedAt: string;
  mode: CleanupMode;
  dryRun: boolean;
  freed: number;
  removed: number;
  skipped: number;
  failed: number;
  undoable: boolean;
  items: ItemOutcome[];
}

export interface DiskInfo {
  name: string;
  mountPoint: string;
  fileSystem: string;
  total: number;
  available: number;
  removable: boolean;
}

export interface Finding {
  scanner: string;
  sentence: string;
  reason: string;
  size: number;
  risk: RiskLevel;
}

export interface Explanation {
  headline: string;
  findings: Finding[];
  safeTotal: number;
  reviewTotal: number;
  nextSteps: string[];
}

export interface ScanOptions {
  minResultSize: number;
  largeFileThreshold: number;
  staleAfterDays: number;
  duplicateMinSize: number;
  maxDepth: number;
  disabledScanners: string[];
  ignoredPaths: string[];
}

export interface ScanHistoryEntry {
  id: string;
  startedAt: string;
  durationMs: number;
  totalItems: number;
  totalSize: number;
  safeSize: number;
}

export interface DiskSnapshot {
  takenAt: string;
  total: number;
  available: number;
}

export interface ScannerStats {
  scanner: string;
  runs: number;
  totalItems: number;
  totalSize: number;
  totalDurationMs: number;
}

/**
 * How protected a path is.
 *
 * Three tiers, not a boolean: automated cleanup refuses anything that is not
 * `allowed`, while the file explorer may remove `caution` items once the user
 * confirms them. `refused` is absolute.
 */
export type Protection =
  | { kind: "allowed" }
  | { kind: "caution"; reason: string }
  | { kind: "refused"; reason: string };

export interface ProtectedEntry {
  path: string;
  reason: string;
  /** True when nothing in SpaceKeeper can remove it; false for personal data. */
  refused: boolean;
}

export interface BrowseEntry {
  name: string;
  path: string;
  kind: EntryKind;
  /** Zero with `measured: false` means "not known yet", not "empty". */
  size: number;
  /** False while the background pass is still walking this directory. */
  measured: boolean;
  fileCount: number;
  /** Fraction of the parent folder's total size, 0–1. */
  share: number;
  lastAccessed: string | null;
  lastModified: string | null;
  protection: Protection;
  isSymlink: boolean;
}

export interface Crumb {
  name: string;
  path: string;
}

export interface DirectoryListing {
  path: string;
  parent: string | null;
  breadcrumbs: Crumb[];
  totalSize: number;
  entries: BrowseEntry[];
  hiddenEntries: number;
  protection: Protection;
}

export type SortBy = "size" | "name" | "modified" | "oldest";

/** One directory size, streamed in after the listing was returned. */
export interface MeasuredEvent {
  /** Listing generation. Anything not matching the current folder is dropped. */
  token: number;
  path: string;
  size: number;
  fileCount: number;
  lastAccessed: string | null;
  lastModified: string | null;
  crossesMount: boolean;
}

/** Error shape returned by every command. */
export interface CommandError {
  kind: string;
  message: string;
}
