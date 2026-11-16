import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/** A small industrial bolt for a container's corners. */
export function FrankenBolt({ className }: { className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        "relative flex h-3 w-3 items-center justify-center rounded-full border border-white/10 bg-gradient-to-br from-slate-700 via-slate-900 to-black shadow-[inset_0_1px_1px_rgba(255,255,255,0.05),0_1px_3px_rgba(0,0,0,0.6)]",
        className,
      )}
    >
      <span className="absolute h-[60%] w-[1.5px] rotate-45 bg-slate-800" />
      <span className="absolute h-[60%] w-[1.5px] -rotate-45 bg-slate-800" />
    </span>
  );
}

/** Hand-drawn sutures between sections. */
export function FrankenStitch({ className }: { className?: string }) {
  return (
    <svg
      aria-hidden="true"
      className={cn("h-3 w-full text-green-500/30", className)}
      viewBox="0 0 100 12"
      preserveAspectRatio="none"
    >
      <path
        d="M5 6 L15 6 M10 1 L10 11 M25 6 L35 6 M30 1 L30 11 M45 6 L55 6 M50 1 L50 11 M65 6 L75 6 M70 1 L70 11 M85 6 L95 6 M90 1 L90 11"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}

/** A glass panel held by four bolts. */
export function FrankenContainer({
  children,
  className,
  bolts = true,
}: {
  children: ReactNode;
  className?: string;
  bolts?: boolean;
}) {
  return (
    <div className={cn("glass-modern relative rounded-2xl", className)}>
      {bolts && (
        <>
          <FrankenBolt className="absolute top-3 left-3" />
          <FrankenBolt className="absolute top-3 right-3" />
          <FrankenBolt className="absolute bottom-3 left-3" />
          <FrankenBolt className="absolute right-3 bottom-3" />
        </>
      )}
      {children}
    </div>
  );
}

/** The micro-label: an instrument's eyebrow. */
export function MicroLabel({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <span className={cn("text-[10px] font-black tracking-[0.3em] text-green-500/80 uppercase", className)}>
      {children}
    </span>
  );
}

/** A heading whose `[bracketed]` words are the one green word. */
export function GreenTitle({ text }: { text: string }) {
  const parts = text.split(/(\[[^\]]+\])/);
  return (
    <>
      {parts.map((part, index) =>
        part.startsWith("[") && part.endsWith("]") ? (
          <span key={index} className="text-animate-green">
            {part.slice(1, -1)}
          </span>
        ) : (
          <span key={index}>{part}</span>
        ),
      )}
    </>
  );
}
