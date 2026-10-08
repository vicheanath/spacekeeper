import { useVirtualizer } from "@tanstack/react-virtual";
import { useRef, type ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * A windowed list.
 *
 * The explorer and the results view can both hold a thousand rows, each with a
 * checkbox, a progress bar, tooltips and an icon. Rendering all of them costs
 * hundreds of milliseconds on open and makes scrolling stutter; rendering only
 * what fits on screen costs the same regardless of list length.
 *
 * Rows are measured dynamically rather than assumed to be a fixed height, since
 * a long path or a wrapped description makes them differ.
 */
export function VirtualList<T>({
  items,
  estimateHeight,
  getKey,
  renderRow,
  className,
  overscan = 8,
}: {
  items: T[];
  /** Starting guess for row height; real heights are measured after mount. */
  estimateHeight: number;
  getKey: (item: T, index: number) => string;
  renderRow: (item: T, index: number) => ReactNode;
  className?: string;
  overscan?: number;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);

  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => estimateHeight,
    overscan,
    getItemKey: (index) => {
      const item = items[index];
      return item === undefined ? index : getKey(item, index);
    },
  });

  return (
    <div ref={scrollRef} className={cn("min-h-0 flex-1 overflow-y-auto", className)}>
      <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
        {virtualizer.getVirtualItems().map((virtualRow) => {
          const item = items[virtualRow.index];
          if (item === undefined) return null;
          return (
            <div
              key={virtualRow.key}
              // `measureElement` reports the real height back, so rows of
              // different sizes still land in the right place.
              ref={virtualizer.measureElement}
              data-index={virtualRow.index}
              className="absolute left-0 top-0 w-full"
              style={{ transform: `translateY(${virtualRow.start}px)` }}
            >
              {renderRow(item, virtualRow.index)}
            </div>
          );
        })}
      </div>
    </div>
  );
}
