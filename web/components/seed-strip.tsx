import type { SeedStrip } from "@/lib/content";
import { cn } from "@/lib/utils";

/**
 * A strip's colour: the stated way on most seeds of both splits, on most of
 * neither, or not consistently (the splits disagree, or a split is even).
 */
export const SEED_TONES = { both: "bg-green-400", neither: "bg-slate-400", mixed: "bg-amber-300" } as const;

/** `k` of `n` seeds as dots, the first `k` lit: "effect e over k of n seeds versus control". */
function Dots({ k, n, tone }: { k: number; n: number; tone: string }) {
  return (
    <span className="inline-flex flex-wrap gap-[3px]" role="img" aria-label={`${k} of ${n} seeds`}>
      {Array.from({ length: n }, (_, index) => (
        <span
          key={index}
          aria-hidden="true"
          className={cn("h-2 w-2 rounded-full", index < k ? tone : "border border-white/20 bg-transparent")}
        />
      ))}
    </span>
  );
}

/** A measure's consistency across paired seeds, discovery beside held-out (experiments/fl048/README.md). */
export function SeedStrips({ strips, className }: { strips: SeedStrip[]; className?: string }) {
  return (
    <dl className={cn("space-y-2.5", className)}>
      {strips.map((strip) => {
        const [dk, dn] = strip.discovery;
        const [hk, hn] = strip.heldOut;
        // Agreement between the splits is what the declared rules ask for (SEED_TONES).
        const tone =
          dk / dn > 0.5 && hk / hn > 0.5 ? SEED_TONES.both : dk / dn < 0.5 && hk / hn < 0.5 ? SEED_TONES.neither : SEED_TONES.mixed;
        return (
          <div key={strip.measure} className="grid grid-cols-1 gap-x-4 gap-y-1 sm:grid-cols-[minmax(0,1fr)_auto_auto]">
            <dt className="text-xs leading-snug text-slate-300">{strip.measure}</dt>
            <dd className="flex items-center gap-2 font-mono text-[11px] text-slate-500 tabular-nums">
              <span className="w-[4.5rem] shrink-0 sm:w-auto">discovery</span>
              <Dots k={dk} n={dn} tone={tone} />
              <span className="text-slate-300">
                {dk}/{dn}
              </span>
            </dd>
            <dd className="flex items-center gap-2 font-mono text-[11px] text-slate-500 tabular-nums">
              <span className="w-[4.5rem] shrink-0 sm:w-auto">held out</span>
              <Dots k={hk} n={hn} tone={tone} />
              <span className="text-slate-300">
                {hk}/{hn}
              </span>
            </dd>
          </div>
        );
      })}
    </dl>
  );
}
