import { useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/lib/api";
import type { BrowseEntry, DirectoryListing, MeasuredEvent, SortBy } from "@/lib/types";

/**
 * Merges streamed directory sizes into a listing.
 *
 * The backend returns a folder immediately and walks its subdirectories
 * afterwards, so sizes arrive one at a time. Three things matter here:
 *
 * * **stale results are dropped.** Every event carries the generation of the
 *   listing it belongs to; navigating away bumps the generation, so work still
 *   in flight for the previous folder cannot rewrite the current one.
 * * **updates are batched.** A folder with 800 subdirectories would otherwise
 *   cause 800 React renders. Events are collected and flushed on an animation
 *   frame, which turns that into a handful.
 * * **sorting waits.** Re-sorting on every arrival makes rows leap around
 *   under the cursor. The order is recomputed only once measuring settles,
 *   unless the user picked a sort that does not depend on size.
 */
export function useMeasuredSizes(
  listing: DirectoryListing | undefined,
  sort: SortBy,
): { entries: BrowseEntry[]; totalSize: number; measuring: boolean } {
  const [measured, setMeasured] = useState<Map<string, MeasuredEvent>>(new Map());
  const [settled, setSettled] = useState(true);

  // The listing this hook is currently merging into, so events for a folder
  // the user has left are ignored.
  const listingPath = listing?.path;
  const pending = useRef<MeasuredEvent[]>([]);
  const frame = useRef<number | null>(null);
  const settleTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    // A new folder starts with nothing merged.
    setMeasured(new Map());
    pending.current = [];
  }, [listingPath]);

  const expected = useMemo(
    () => listing?.entries.filter((entry) => !entry.measured && !entry.isSymlink).length ?? 0,
    [listing],
  );

  useEffect(() => {
    setSettled(expected === 0);
  }, [expected, listingPath]);

  useEffect(() => {
    const paths = new Set(listing?.entries.map((entry) => entry.path) ?? []);

    const flush = () => {
      frame.current = null;
      const batch = pending.current;
      pending.current = [];
      if (batch.length === 0) return;

      setMeasured((current) => {
        const next = new Map(current);
        for (const event of batch) next.set(event.path, event);
        return next;
      });

      // Consider measuring finished once events stop arriving, rather than
      // counting them — a folder that fails to measure would otherwise leave
      // the list permanently unsorted.
      if (settleTimer.current) clearTimeout(settleTimer.current);
      settleTimer.current = setTimeout(() => setSettled(true), 250);
    };

    const unlisten = api.onMeasured((event) => {
      if (!paths.has(event.path)) return;
      pending.current.push(event);
      frame.current ??= requestAnimationFrame(flush);
    });

    return () => {
      void unlisten.then((stop) => stop());
      if (frame.current !== null) cancelAnimationFrame(frame.current);
      if (settleTimer.current) clearTimeout(settleTimer.current);
    };
  }, [listing]);

  return useMemo(() => {
    const base = listing?.entries ?? [];
    const merged = base.map((entry) => {
      const update = measured.get(entry.path);
      if (!update) return entry;
      return {
        ...entry,
        size: update.size,
        fileCount: update.fileCount,
        lastAccessed: update.lastAccessed,
        lastModified: update.lastModified,
        measured: true,
        protection: update.crossesMount
          ? ({
              kind: "refused",
              reason: "another disk is mounted inside this folder",
            } as const)
          : entry.protection,
      };
    });

    const totalSize = merged.reduce((sum, entry) => sum + entry.size, 0);

    // Shares are only meaningful against a finished total. Computing them
    // early makes a 12 KB file briefly read as most of the folder, because at
    // that moment the files are all that has been measured.
    const withShares = merged.map((entry) => ({
      ...entry,
      share: settled && totalSize > 0 ? entry.size / totalSize : 0,
    }));

    // Size ordering is only meaningful once the sizes exist. Holding the order
    // still until then keeps rows from jumping under the pointer.
    const shouldSort = sort !== "size" || settled;
    if (!shouldSort) return { entries: withShares, totalSize, measuring: !settled };

    const sorted = [...withShares].sort((a, b) => {
      switch (sort) {
        case "name":
          return a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
        case "modified":
          return (b.lastModified ?? "").localeCompare(a.lastModified ?? "");
        case "oldest":
          return (a.lastAccessed ?? a.lastModified ?? "").localeCompare(
            b.lastAccessed ?? b.lastModified ?? "",
          );
        default:
          return b.size - a.size || a.name.localeCompare(b.name);
      }
    });

    return { entries: sorted, totalSize, measuring: !settled };
  }, [listing, measured, sort, settled]);
}
