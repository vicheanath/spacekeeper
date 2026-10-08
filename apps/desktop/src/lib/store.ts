/**
 * Client state that TanStack Query should not own.
 *
 * The split is deliberate: anything the backend is the source of truth for
 * (reports, history, settings) lives in Query. Only genuinely local, ephemeral
 * state lives here — what the user has ticked, live scan progress, the theme.
 */

import { create } from "zustand";

import type { ScanProgress, ScanReport } from "./types";

type Theme = "system" | "light" | "dark";

interface ScanState {
  /** Whether a scan is currently running. */
  running: boolean;
  paused: boolean;
  /** Latest event per scanner, keyed by scanner id. */
  progress: Record<string, ScanProgress>;
  /** Scanners that have reached a terminal phase. */
  finished: Set<string>;
  total: number;

  /** Ids of results the user has ticked. */
  selection: Set<string>;

  theme: Theme;

  beginScan: () => void;
  applyProgress: (event: ScanProgress) => void;
  endScan: () => void;
  setPaused: (paused: boolean) => void;

  toggle: (id: string) => void;
  setSelection: (ids: string[]) => void;
  clearSelection: () => void;

  setTheme: (theme: Theme) => void;
}

const THEME_KEY = "spacekeeper.theme";

function readTheme(): Theme {
  if (typeof localStorage === "undefined") return "system";
  const stored = localStorage.getItem(THEME_KEY);
  return stored === "light" || stored === "dark" ? stored : "system";
}

/**
 * Apply the theme to the document root.
 *
 * shadcn's dark variant keys off a `dark` class, so `system` is resolved here
 * against the OS preference rather than being handed to CSS.
 */
export function applyTheme(theme: Theme): void {
  if (typeof document === "undefined") return;
  const dark =
    theme === "dark" ||
    (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", dark);
}

/** Keep a `system` theme in sync when the OS preference changes. */
export function watchSystemTheme(): () => void {
  if (typeof window === "undefined") return () => {};
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  const listener = () => {
    if (useAppStore.getState().theme === "system") applyTheme("system");
  };
  media.addEventListener("change", listener);
  return () => media.removeEventListener("change", listener);
}

export const useAppStore = create<ScanState>((set) => ({
  running: false,
  paused: false,
  progress: {},
  finished: new Set(),
  total: 0,
  selection: new Set(),
  theme: readTheme(),

  beginScan: () =>
    set({ running: true, paused: false, progress: {}, finished: new Set(), total: 0 }),

  applyProgress: (event) =>
    set((state) => {
      const finished = new Set(state.finished);
      if (event.phase === "finished" || event.phase === "skipped" || event.phase === "failed") {
        finished.add(event.scanner);
      }
      return {
        progress: { ...state.progress, [event.scanner]: event },
        finished,
        total: event.scannersTotal || state.total,
      };
    }),

  endScan: () => set({ running: false, paused: false }),
  setPaused: (paused) => set({ paused }),

  toggle: (id) =>
    set((state) => {
      const selection = new Set(state.selection);
      if (selection.has(id)) selection.delete(id);
      else selection.add(id);
      return { selection };
    }),

  setSelection: (ids) => set({ selection: new Set(ids) }),
  clearSelection: () => set({ selection: new Set() }),

  setTheme: (theme) => {
    if (typeof localStorage !== "undefined") localStorage.setItem(THEME_KEY, theme);
    applyTheme(theme);
    set({ theme });
  },
}));

/**
 * Everything the UI needs to summarise a selection, computed from the report
 * so the totals shown always match the items that will actually be sent.
 */
export function summarizeSelection(report: ScanReport | undefined, selection: Set<string>) {
  if (!report) return { count: 0, size: 0, hasReview: false };
  let size = 0;
  let count = 0;
  let hasReview = false;
  for (const result of report.results) {
    if (!selection.has(result.id)) continue;
    count += 1;
    size += result.size;
    if (result.risk !== "safe") hasReview = true;
  }
  return { count, size, hasReview };
}
