import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

import { GreenTitle, MicroLabel } from "@/components/franken";
import { Reveal } from "@/components/reveal";
import { cn } from "@/lib/utils";

/**
 * A chapter of the page: a sticky sidebar header (eyebrow, one-green-word
 * title, kicker) beside its content on wide screens, stacked on narrow ones.
 */
export function Section({
  id,
  icon: Icon,
  eyebrow,
  title,
  kicker,
  children,
  className,
}: {
  id?: string;
  icon: LucideIcon;
  eyebrow: string;
  title: string;
  kicker?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  const headingId = id ? `${id}-heading` : undefined;
  return (
    <section
      id={id}
      aria-labelledby={headingId}
      className={cn("relative mx-auto max-w-7xl px-4 py-16 sm:px-6 md:py-28 lg:py-36", className)}
    >
      <div className="grid grid-cols-1 items-start gap-10 lg:grid-cols-12 lg:gap-20">
        <Reveal className="space-y-6 lg:sticky lg:top-28 lg:col-span-4">
          <div className="inline-flex items-center gap-3">
            <span className="h-px w-8 bg-green-500/40" />
            <MicroLabel>{eyebrow}</MicroLabel>
          </div>
          <div className="flex items-center gap-4">
            <span className="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl border border-green-500/20 bg-green-500/5 text-green-400">
              <Icon className="h-5 w-5" aria-hidden="true" />
            </span>
            <h2 id={headingId} className="text-3xl text-white sm:text-4xl">
              <GreenTitle text={title} />
            </h2>
          </div>
          {kicker && <div className="max-w-md text-base leading-relaxed text-slate-400">{kicker}</div>}
        </Reveal>
        <div className="min-w-0 lg:col-span-8">{children}</div>
      </div>
    </section>
  );
}
