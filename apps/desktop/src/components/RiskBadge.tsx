import { CircleAlert, Lock, ShieldCheck } from "lucide-react";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { RiskLevel } from "@/lib/types";
import { cn, RISK_LABEL, RISK_MEANING } from "@/lib/utils";

const STYLES: Record<RiskLevel, string> = {
  safe: "bg-safe/10 text-safe ring-safe/25",
  review: "bg-warn/10 text-warn ring-warn/25",
  dangerous: "bg-risky/10 text-risky ring-risky/25",
};

const ICONS: Record<RiskLevel, typeof ShieldCheck> = {
  safe: ShieldCheck,
  review: CircleAlert,
  dangerous: Lock,
};

/**
 * The risk badge.
 *
 * Always carries its label, never colour alone — and the tooltip says what the
 * level *means* rather than repeating the word. A user should never have to
 * learn our vocabulary to use the app safely.
 */
export function RiskBadge({ risk, className }: { risk: RiskLevel; className?: string }) {
  const Icon = ICONS[risk];
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <span
            className={cn(
              "inline-flex cursor-help items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium ring-1 ring-inset",
              STYLES[risk],
              className,
            )}
            tabIndex={0}
          />
        }
      >
        <Icon className="size-3" aria-hidden />
        {RISK_LABEL[risk]}
      </TooltipTrigger>
      <TooltipContent className="max-w-xs">{RISK_MEANING[risk]}</TooltipContent>
    </Tooltip>
  );
}
