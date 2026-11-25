import {
  ArrowDown,
  ArrowRight,
  Ban,
  CircleCheck,
  CircleDashed,
  CircleDot,
  Equal,
  EqualNot,
  ExternalLink,
  Map as MapIcon,
  ShieldCheck,
  Terminal,
} from "lucide-react";
import type { CSSProperties } from "react";

import { CopyCommand } from "@/components/copy-command";
import { BrainAnatomy } from "@/components/explain/brain";
import { BranchDiagram } from "@/components/explain/branches";
import { ControlsTrio } from "@/components/explain/controls-trio";
import { FixtureBench } from "@/components/explain/fixture-bench";
import { LifeCycle } from "@/components/explain/life-cycle";
import { Miniature } from "@/components/explain/miniature";
import { ScaleCalculator } from "@/components/explain/scale";
import { StudioKeys } from "@/components/explain/studio-keys";
import { TickPipeline } from "@/components/explain/tick-pipeline";
import { MechanismStory, SharedRules } from "@/components/explain/v2-atlas";
import { V2Explorer } from "@/components/explain/v2-explorer";
import { IndependentWeights } from "@/components/explain/weights";
import { ZoomLadder } from "@/components/explain/zoom-ladder";
import { FilmFacts } from "@/components/film-facts";
import { FilmRecords } from "@/components/film-records";
import { FilmReel } from "@/components/film-reel";
import { GreenTitle, MicroLabel, Reticle } from "@/components/franken";
import { Reveal } from "@/components/reveal";
import { RevealController } from "@/components/reveal-controller";
import { SEED_TONES, SeedStrips } from "@/components/seed-strip";
import { ChapterBlock, Section } from "@/components/section";
import { SiteHeader } from "@/components/site-header";
import { SkipOffscreen } from "@/components/skip-offscreen";
import { BundlePanels, Residuals, TakeOneAway } from "@/components/v2-sections";
import {
  branchObservation,
  type Chapter,
  chapters,
  cliSurface,
  commands,
  composes,
  gates,
  type GateState,
  heroStats,
  hypotheses,
  inheritance,
  inspiration,
  milestones,
  presets,
  priorWork,
  shortcuts,
  siteConfig,
  stack,
  type Verdict,
  worldModel,
} from "@/lib/content";
import { film } from "@/lib/media";
import { cn, grouped } from "@/lib/utils";

const { video } = film;

const gateStyle: Record<GateState, { label: string; className: string; Icon: typeof CircleCheck }> = {
  built: { label: "Built and tested", className: "text-green-300 border-green-500/30 bg-green-500/10", Icon: CircleCheck },
  partial: { label: "Partial", className: "text-amber-200 border-amber-400/30 bg-amber-400/10", Icon: CircleDot },
  open: { label: "Not yet", className: "text-slate-300 border-white/15 bg-white/5", Icon: CircleDashed },
};

const verdictStyle: Record<Verdict, string> = {
  met: "text-green-300 border-green-500/30 bg-green-500/10",
  narrow: "text-amber-200 border-amber-400/30 bg-amber-400/10",
  not: "text-rose-200 border-rose-400/30 bg-rose-400/10",
};

const gateById = new Map(gates.map((gate) => [gate.id, gate]));

/** A chapter by id; the page names only chapters lib/content.ts defines. */
function chapter(id: string): Chapter {
  const found = chapters.find((entry) => entry.id === id);
  if (!found) throw new Error(`no chapter ${id}`);
  return found;
}

/** A section's number within its chapter, "03.2", from the chapter's own list. */
function index(chapterId: string, sectionId: string): string {
  const owner = chapter(chapterId);
  const at = owner.sections.findIndex((section) => section.id === sectionId);
  if (at < 0) throw new Error(`chapter ${chapterId} has no section ${sectionId}`);
  return `${owner.number}.${at + 1}`;
}

const link = "text-green-300 underline decoration-green-300/30 underline-offset-4 transition-colors hover:decoration-green-300";

/** Run it yourself: the commands shown open; the rest fold away. */
const FIRST_COMMANDS = 4;

/** The hero's index of what the page shows, each a jump to it. */
const heroChips = [
  { label: "One brain per organism", href: "#brains" },
  { label: "One energy ledger", href: "#world" },
  { label: "Replay and branch", href: "#replay" },
  { label: "21 v2 switches", href: "#ch-v2" },
  { label: "Controls that remove a cause", href: "#controls" },
];

export default function Home() {
  return (
    <>
      <SiteHeader />
      <RevealController />
      <SkipOffscreen />
      <main id="main" className="relative isolate">
        <span id="top" aria-hidden="true" className="absolute top-0" />

        {/* Hero: the specimen first. On a phone the film is the first thing on screen; on a wide screen it
            fills the right of the first view, feathered into the dark, with the words on the left. */}
        <section data-hero aria-labelledby="hero-heading" className="relative isolate overflow-hidden">
          <div aria-hidden="true" className="field-light absolute inset-0 -z-10" />
          <div aria-hidden="true" className="lattice absolute inset-0 -z-10" />
          <div className="mx-auto grid max-w-[100rem] grid-cols-1 lg:min-h-[100svh] lg:grid-cols-[minmax(0,0.92fr)_minmax(0,1.08fr)] lg:items-center">
            <div
              className="rise-in relative order-1 pt-[4.25rem] [--rise-delay:80ms] lg:order-2 lg:pt-24 lg:pr-8 lg:pb-8 xl:pr-12"
              style={{ "--film-aspect": video.width / video.height } as CSSProperties}
            >
              <FilmReel
                feather
                idPrefix="hero"
                className="mx-auto w-full lg:max-w-[min(100%,calc((100svh-11rem)*var(--film-aspect)))]"
              />
            </div>
            <div className="rise-in relative z-10 order-2 px-5 pt-9 pb-14 sm:px-8 lg:order-1 lg:py-24 lg:pr-4 lg:pl-12 xl:pl-20">
              <p className="flex items-center gap-3">
                <span aria-hidden="true" className="sphere h-3 w-3 [--sphere:#4ade80]" />
                <span className="font-mono text-[11px] font-medium tracking-[0.22em] text-green-300/90 uppercase">
                  Artificial life · Rust · bit-exact
                </span>
              </p>
              <h1 id="hero-heading" className="display-1 mt-6 text-white">
                <GreenTitle lit text="Evolution, [bit for bit]." />
              </h1>
              <p className="lede mt-7 max-w-[36rem]">
                Every organism here has its own heritable neural network. They feed, move, split and die through one
                shared field, nothing is scripted or trained, and every replay reaches the same world bit for bit.
              </p>
              <ul className="mt-6 flex max-w-[36rem] flex-wrap gap-2 font-mono text-[11px] text-slate-300">
                {heroChips.map((chip) => (
                  <li key={chip.label}>
                    <a
                      href={chip.href}
                      className="inline-flex min-h-8 items-center gap-1.5 rounded-full border border-white/10 bg-black/30 px-3 transition-colors hover:border-green-400/50 hover:text-green-200"
                    >
                      <span aria-hidden="true" className="h-1.5 w-1.5 rounded-full bg-green-400" />
                      {chip.label}
                    </a>
                  </li>
                ))}
              </ul>
              <div className="mt-9 flex flex-wrap gap-3">
                <a
                  href="#zoom"
                  className="inline-flex min-h-12 items-center gap-2 rounded-2xl bg-green-500 px-5 text-[15px] font-extrabold text-black shadow-[0_0_40px_-10px_rgba(34,197,94,0.8)] transition-colors hover:bg-green-400"
                >
                  Zoom into one organism <ArrowDown className="h-4 w-4" aria-hidden="true" />
                </a>
                <a
                  href="#alive"
                  className="inline-flex min-h-12 items-center gap-2 rounded-2xl border border-white/15 bg-white/[0.02] px-5 text-[15px] font-bold text-white transition-colors hover:border-green-400/50 hover:text-green-300"
                >
                  Run a world yourself
                </a>
              </div>
              <p className="mt-6 text-sm text-slate-500">
                Open source on{" "}
                <a className={link} href={siteConfig.github}>
                  GitHub
                </a>
                .
              </p>
            </div>
          </div>
        </section>

        {/* The specification strip: four facts, each with its own qualification. */}
        <section aria-label="In four figures" className="relative border-y border-white/[0.06] bg-black/20">
          <dl className="mx-auto grid max-w-7xl grid-cols-2 lg:grid-cols-4">
            {heroStats.map((stat, at) => (
              <div
                key={stat.label}
                className={cn(
                  "min-w-0 border-white/[0.06] px-5 py-7 sm:px-7 lg:py-9",
                  at % 2 === 1 && "border-l",
                  at >= 2 && "border-t lg:border-t-0",
                  at === 2 && "lg:border-l",
                )}
              >
                <dt className="font-mono text-[10px] font-medium tracking-[0.18em] text-green-300/80 uppercase">{stat.label}</dt>
                <dd className="mt-2">
                  <span className="block text-[1.7rem] font-extrabold tracking-[-0.035em] text-white tabular-nums sm:text-[2.1rem]">
                    {stat.value}
                  </span>
                  <span className="mt-2 block text-[13px] leading-snug text-slate-400">{stat.note}</span>
                </dd>
              </div>
            ))}
          </dl>
        </section>

        {/* ─── 01 · The world ─────────────────────────────────────────── */}
        <ChapterBlock chapter={chapter("world")}>
          <section id="zoom" aria-label="From the world to one brain" className="relative mx-auto max-w-7xl px-4 py-14 sm:px-6 md:py-20">
            <ZoomLadder index={index("world", "zoom")} />
          </section>

          <Section
            id="what"
            index={index("world", "what")}
            layout="wide"
            eyebrow="One organism, one tick"
            title="A life in [nine] steps."
            kicker="Sense, decide, feed, move, emit, divide, die: one organism's turn, in the shared field."
          >
            <Reveal>
              <LifeCycle />
            </Reveal>
          </Section>

          <Section
            id="alive"
            index={index("world", "alive")}
            layout="wide"
            eyebrow="A toy world, live"
            title="Run a small world [yourself]."
            kicker="A toy world in your browser, about a thousand organisms under four of the rules. Select one, brush energy in, replay from the seed."
          >
            <Reveal>
              <Miniature />
            </Reveal>
          </Section>
        </ChapterBlock>

        {/* ─── 02 · Why it is different ───────────────────────────────── */}
        <ChapterBlock chapter={chapter("different")}>
          <Section
            id="different"
            index={index("different", "different")}
            eyebrow="Four shortcuts"
            title="Each one, [forbidden]."
            kicker="Each can make a gorgeous video of nothing evolving. The rules forbid all four; the tests enforce them."
          >
            <Reveal>
              <div className="panel overflow-hidden rounded-[1.75rem]">
                <div className="hidden grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)] border-b border-white/[0.06] md:grid">
                  <p className="flex items-center gap-2 px-7 py-4">
                    <Ban className="h-3.5 w-3.5 text-rose-300/80" aria-hidden="true" />
                    <MicroLabel className="text-rose-300/80">The shortcut</MicroLabel>
                  </p>
                  <p className="flex items-center gap-2 border-l border-white/[0.06] px-7 py-4">
                    <ShieldCheck className="h-3.5 w-3.5 text-green-400" aria-hidden="true" />
                    <MicroLabel>FrankenLife&apos;s rule</MicroLabel>
                  </p>
                </div>
                <ol className="ruled">
                  {shortcuts.map((entry, at) => (
                    <li key={entry.shortcut} className="grid grid-cols-1 md:grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)]">
                      <div className="px-6 pt-6 pb-3 sm:px-7 md:py-7">
                        <p className="font-mono text-[11px] text-rose-300/70 tabular-nums">
                          0{at + 1}
                          <span className="md:hidden"> · the shortcut</span>
                        </p>
                        <h4 className="mt-2 text-lg text-slate-200 line-through decoration-rose-400/70 decoration-2">
                          {entry.shortcut}
                        </h4>
                        <p className="mt-1.5 text-sm leading-relaxed text-slate-400">{entry.temptation}</p>
                      </div>
                      <div className="bg-green-500/[0.025] px-6 pt-3 pb-6 sm:px-7 md:border-l md:border-white/[0.06] md:py-7">
                        <p className="mb-2 flex items-center gap-2 md:hidden">
                          <ShieldCheck className="h-3.5 w-3.5 text-green-400" aria-hidden="true" />
                          <MicroLabel>The rule</MicroLabel>
                        </p>
                        <p className="text-[15px] leading-relaxed text-slate-200">{entry.rule}</p>
                        <p className="mt-3 flex flex-wrap gap-1.5">
                          {entry.refs.map((ref) => (
                            <span
                              key={ref}
                              className="rounded-md border border-white/10 bg-black/30 px-2 py-0.5 font-mono text-[11px] text-slate-400"
                            >
                              {ref}
                            </span>
                          ))}
                        </p>
                      </div>
                    </li>
                  ))}
                </ol>
              </div>
            </Reveal>

            {/* What the field already has, beside the four things FrankenLife puts together at once. */}
            <Reveal className="mt-5">
              <div className="panel relative rounded-[1.75rem] p-6 sm:p-8">
                <Reticle inset={9} size={12} />
                <MicroLabel>Four things at once</MicroLabel>
                <ol className="mt-5 grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4">
                  {composes.map((entry, at) => (
                    <li key={entry.title} className="rounded-2xl border border-white/[0.08] bg-black/25 p-4">
                      <span className="flex h-7 w-7 items-center justify-center rounded-lg border border-green-500/30 bg-green-500/10 font-mono text-xs font-bold text-green-300">
                        {at + 1}
                      </span>
                      <p className="mt-3 text-[15px] leading-snug font-bold text-white">{entry.title}</p>
                      <p className="mt-1.5 text-[13px] leading-snug text-slate-400">{entry.body}</p>
                    </li>
                  ))}
                </ol>
                <p className="mt-5 flex flex-wrap items-center gap-2 text-xs text-slate-500">
                  <span>Related work:</span>
                  {priorWork.map((entry) => (
                    <span
                      key={entry.name}
                      title={entry.shows}
                      className="rounded-full border border-white/10 px-2.5 py-0.5 font-mono text-[11px] text-slate-400"
                    >
                      {entry.name}
                    </span>
                  ))}
                </p>
              </div>
            </Reveal>
          </Section>

          <Section
            id="controls"
            index={index("different", "controls")}
            layout="wide"
            eyebrow="The model and its controls"
            title="Remove a cause, lose the [structure]."
            kicker="Without the networks' emission the tissue becomes a haze; without inheritance the world dwindles to scatter."
          >
            <Reveal>
              <ControlsTrio />
            </Reveal>
          </Section>

          <Section
            id="film"
            index={index("different", "film")}
            layout="wide"
            eyebrow="Provenance"
            title="What you are [watching]."
            kicker="Every frame is flife render's, from the committed world at its tick. The same commands reach the same digests on any machine."
          >
            <Reveal>
              <FilmFacts />
            </Reveal>
            <Reveal className="mt-5">
              <FilmRecords />
            </Reveal>
          </Section>
        </ChapterBlock>

        {/* ─── 03 · How it works ──────────────────────────────────────── */}
        <ChapterBlock chapter={chapter("machinery")}>
          <Section
            id="brains"
            index={index("machinery", "brains")}
            layout="wide"
            eyebrow="Independent controllers"
            title="Every organism, its [own] brain."
            kicker="Each organism runs its own dense network on what it senses. No two share a parameter, nothing is trained by gradient, and the weights change only when a child inherits a mutated copy."
          >
            <div className="space-y-5">
              <Reveal>
                <BrainAnatomy />
              </Reveal>
              <Reveal>
                <IndependentWeights />
              </Reveal>
            </div>
          </Section>

          <Section
            id="world"
            index={index("machinery", "world")}
            eyebrow="FieldEcologyV1"
            title="The model, [explicitly]."
            kicker="An original, explicit rule set: a periodic field of energy and chemistry, and organisms that can only touch each other through it."
          >
            <Reveal>
              <dl className="panel grid grid-cols-1 overflow-hidden rounded-[1.75rem] sm:grid-cols-2">
                {worldModel.map((part, at) => (
                  <div
                    key={part.name}
                    className={cn(
                      "border-white/[0.06] p-6 sm:p-7",
                      at > 0 && "border-t",
                      at === 1 && "sm:border-t-0",
                      at % 2 === 1 && "sm:border-l",
                    )}
                  >
                    <dt className="font-mono text-xs font-medium tracking-[0.18em] text-[color:var(--chapter)] uppercase">
                      {part.name}
                    </dt>
                    <dd className="mt-2.5 text-[15px] leading-relaxed text-slate-300">{part.body}</dd>
                  </div>
                ))}
              </dl>
            </Reveal>
            <Reveal className="mt-5">
              <div className="panel panel-strong relative rounded-[1.75rem] p-6 sm:p-8">
                <Reticle inset={9} size={12} />
                <MicroLabel>The ledger</MicroLabel>
                <p className="mt-5 overflow-x-auto font-mono text-[15px] whitespace-nowrap text-green-100 sm:text-lg">
                  𝓔ₙ₊₁ − 𝓔ₙ = injection − removal − dissipation + numerical residual
                </p>
                <p className="mt-4 text-sm leading-relaxed text-slate-400">
                  𝓔 is the energy in every organism plus the energy in every cell. Emission, absorption and a
                  child&apos;s endowment only move it. No correction may add energy to make the equation balance.
                </p>
              </div>
            </Reveal>
          </Section>

          <Section
            id="rules"
            index={index("machinery", "rules")}
            layout="wide"
            eyebrow="Appendix C, playable"
            title="Check it [by hand]."
            kicker="Each rule has a plausible-looking wrong version, and the plan pins each one with a known answer small enough to compute by hand. Change the numbers and the page recomputes the rule beside its wrong twin."
          >
            <Reveal>
              <FixtureBench />
            </Reveal>
          </Section>

          <Section
            id="tick"
            index={index("machinery", "tick")}
            layout="wide"
            eyebrow="Ten phases"
            title="One [tick]."
            kicker="Each tick is prepared as a candidate against the committed world and published whole, or not at all. Cancel it part way and nothing has changed."
          >
            <Reveal>
              <TickPipeline />
            </Reveal>
          </Section>
        </ChapterBlock>

        {/* ─── 04 · Version 2 ─────────────────────────────────────────── */}
        <ChapterBlock chapter={chapter("v2")}>
          <Section
            id="bundles"
            index={index("v2", "bundles")}
            layout="wide"
            eyebrow="field-ecology-v2 · two presets"
            title="Two new [worlds]."
            kicker="One model, 21 switches set at founding, two presets: a tissue whose bodies touch, and a supercolony where nothing touches. All off, v2 is v1 bit for bit."
          >
            <BundlePanels />
          </Section>

          <Section
            id="tissue"
            index={index("v2", "tissue")}
            layout="wide"
            eyebrow="Bundle A · seven of its own"
            title="How a tissue [forms]."
            kicker="The tissue's seven own mechanisms in six steps, each with its rule on the preset's numbers and what changed without it."
          >
            <Reveal>
              <MechanismStory story="tissue" />
            </Reveal>
          </Section>

          <Section
            id="supercolony"
            index={index("v2", "supercolony")}
            layout="wide"
            eyebrow="Bundle B · nothing touches"
            title="Held together by the [field]."
            kicker="Its organisms are points that never push or pull, so whatever holds a colony together passes through the field."
          >
            <Reveal>
              <MechanismStory story="supercolony" />
            </Reveal>
          </Section>

          <Section
            id="shared"
            index={index("v2", "shared")}
            layout="wide"
            eyebrow="Both presets, or neither"
            title="The other [switches]."
            kicker="On in both worlds: what a body costs to keep, what it may spend, and where its deposits land."
          >
            <Reveal>
              <SharedRules />
            </Reveal>
          </Section>

          <Section
            id="ablations"
            index={index("v2", "ablations")}
            layout="wide"
            eyebrow="Stage-2 · 104 runs"
            title="Take one [away]."
            kicker={
              <>
                13 conditions × 4 seeds (2 held out) × 2 presets, 2,048 ticks each.{" "}
                <a className={link} href={siteConfig.v2Report}>
                  Every run
                </a>
                .
              </>
            }
          >
            <Reveal>
              <TakeOneAway />
            </Reveal>
          </Section>

          <Section
            id="configure"
            index={index("v2", "configure")}
            layout="wide"
            eyebrow="Founding-time switches"
            title="Compose your [own]."
            kicker="Pick switches: see what each adds to every network's inputs and outputs, and the command that founds the world."
          >
            <Reveal>
              <V2Explorer />
            </Reveal>
          </Section>

          <Section
            id="unshown"
            index={index("v2", "unshown")}
            layout="wide"
            eyebrow="Residuals"
            title="What is not [shown] yet."
            kicker="What went against the hypotheses, and what has not been run, from the report's own list."
          >
            <Reveal>
              <Residuals />
            </Reveal>
          </Section>
        </ChapterBlock>

        {/* ─── 05 · What it can show ──────────────────────────────────── */}
        <ChapterBlock chapter={chapter("evidence")}>
          <Section
            id="replay"
            index={index("evidence", "replay")}
            layout="wide"
            eyebrow="Checkpoints · resume · branches"
            title="Rewind and [branch]."
            kicker="Kill a run and resume it: same world. Branch it: the control matches the parent exactly, so any difference is the intervention's."
          >
            <div className="space-y-5">
              <Reveal>
                <BranchDiagram />
              </Reveal>
              <Reveal>
                <div className="panel panel-strong relative rounded-[1.75rem] p-6 sm:p-8">
                  <Reticle inset={9} size={12} />
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <MicroLabel>We ran it</MicroLabel>
                    <span className="rounded-full border border-green-500/30 bg-green-500/10 px-2.5 py-0.5 font-mono text-[10px] font-medium tracking-[0.12em] text-green-300 uppercase">
                      Observed · {branchObservation.date}
                    </span>
                  </div>
                  <p className="mt-4 max-w-4xl text-[15px] leading-relaxed text-slate-300">
                    Branched at tick {grouped(branchObservation.from)}, one branch brushed ({branchObservation.brush}),
                    all three run to tick {grouped(branchObservation.tick)}:
                  </p>
                  <ul className="mt-5 space-y-2">
                    {branchObservation.digests.map((entry) => (
                      <li
                        key={entry.name}
                        className={cn(
                          "grid grid-cols-1 items-center gap-x-4 gap-y-1 rounded-xl border px-4 py-3 sm:grid-cols-[10rem_minmax(0,1fr)_auto]",
                          entry.same ? "border-green-500/15 bg-green-500/[0.04]" : "border-rose-400/20 bg-rose-500/[0.05]",
                        )}
                      >
                        <span className="text-sm font-semibold text-white">{entry.name}</span>
                        <code className="min-w-0 font-mono text-xs break-all text-slate-300">{entry.digest}</code>
                        <span
                          className={cn(
                            "inline-flex items-center gap-1.5 justify-self-start text-xs font-bold sm:justify-self-end",
                            entry.same ? "text-green-300" : "text-rose-300",
                          )}
                        >
                          {entry.same ? (
                            <Equal className="h-3.5 w-3.5" aria-hidden="true" />
                          ) : (
                            <EqualNot className="h-3.5 w-3.5" aria-hidden="true" />
                          )}
                          {entry.same ? "the parent's world" : "a different world"}
                        </span>
                      </li>
                    ))}
                  </ul>
                  <p className="mt-4 text-xs leading-relaxed text-slate-500">
                    SHA-256 world digests; {branchObservation.host}, flife {branchObservation.binary}….{" "}
                    <a className={link} href="#try">
                      The commands
                    </a>
                    .
                  </p>
                </div>
              </Reveal>
            </div>
          </Section>

          <Section
            id="science"
            index={index("evidence", "science")}
            layout="wide"
            eyebrow="FL-048 · declared before running"
            title="What the experiments [found]."
            kicker={
              <>
                Rules written before the runs, held-out seeds kept apart, failures kept. One strong result, two
                unsupported.{" "}
                <a className={link} href={siteConfig.fl048}>
                  The record
                </a>
                .
              </>
            }
          >
            <Reveal>
              <div className="panel panel-strong relative overflow-hidden rounded-[1.75rem] p-6 sm:p-9">
                <Reticle inset={9} size={12} />
                <div className="grid grid-cols-1 gap-8 lg:grid-cols-12 lg:gap-12">
                  <div className="lg:col-span-5">
                    <MicroLabel>The inheritance control · the strongest result</MicroLabel>
                    <h4 className="mt-4 text-2xl font-extrabold tracking-[-0.03em] text-white sm:text-[2rem] sm:leading-[1.1]">
                      {inheritance.title}
                    </h4>
                    <p className="mt-4 text-[15px] leading-relaxed text-slate-300">{inheritance.body}</p>
                    <p className="mt-5 inline-flex rounded-full border border-green-500/30 bg-green-500/10 px-3 py-1 font-mono text-[11px] font-medium text-green-300">
                      8 of 8 discovery seeds · 8 of 8 held out
                    </p>
                  </div>
                  <div className="lg:col-span-7">
                    {/* slop-ok(S12-three-cards): the record states three figures for this control; the grid follows the data. */}
                    <dl className="grid grid-cols-1 gap-3 sm:grid-cols-3">
                      {inheritance.figures.map((figure) => (
                        <div key={figure.label} className="flex flex-col rounded-2xl border border-white/[0.08] bg-black/30 p-5">
                          {/* Label first for a screen reader (a group is dt then dd); the figure first on screen. */}
                          <dt className="order-2 mt-2 text-sm text-slate-300">{figure.label}</dt>
                          <dd className="order-1 font-mono text-3xl font-medium tracking-[-0.03em] text-green-200 tabular-nums">
                            {figure.value}
                          </dd>
                          <dd className="order-3 mt-1 text-xs leading-snug text-slate-500">{figure.note}</dd>
                        </div>
                      ))}
                    </dl>
                    <SeedStrips className="mt-6" strips={inheritance.strips} />
                    <p className="mt-5 text-xs leading-relaxed text-slate-500">Against the control, paired by seed: {inheritance.scope}</p>
                  </div>
                </div>
              </div>
            </Reveal>
            <Reveal className="mt-5">
              <ol className="panel ruled overflow-hidden rounded-[1.75rem]">
                {hypotheses.map((hypothesis) => (
                  <li key={hypothesis.id} className="grid grid-cols-1 gap-x-10 gap-y-3 p-6 sm:p-7 lg:grid-cols-12">
                    <div className="lg:col-span-5">
                      <div className="flex flex-wrap items-center gap-3">
                        <span className="font-mono text-sm font-medium text-[color:var(--chapter)]">{hypothesis.id}</span>
                        <span
                          className={cn(
                            "rounded-full border px-2.5 py-0.5 font-mono text-[10px] font-medium tracking-[0.1em] uppercase",
                            verdictStyle[hypothesis.verdict],
                          )}
                        >
                          {hypothesis.label}
                        </span>
                      </div>
                      <h4 className="mt-2.5 text-lg text-white">{hypothesis.name}</h4>
                    </div>
                    <div className="lg:col-span-7">
                      <p className="text-sm leading-relaxed text-slate-400">{hypothesis.result}</p>
                      <SeedStrips className="mt-4" strips={hypothesis.strips} />
                    </div>
                  </li>
                ))}
              </ol>
            </Reveal>
            <p className="mt-4 flex flex-wrap items-center gap-x-5 gap-y-2 text-xs text-slate-500">
              <span>Each dot is one seed on which the measure went the stated way.</span>
              <span className="inline-flex items-center gap-1.5">
                <span className={cn("h-2 w-2 rounded-full", SEED_TONES.both)} /> most seeds of both splits
              </span>
              <span className="inline-flex items-center gap-1.5">
                <span className={cn("h-2 w-2 rounded-full", SEED_TONES.mixed)} /> the splits disagree or split evenly
              </span>
              <span className="inline-flex items-center gap-1.5">
                <span className={cn("h-2 w-2 rounded-full", SEED_TONES.neither)} /> most seeds of neither
              </span>
            </p>
          </Section>

          <Section
            id="scale"
            index={index("evidence", "scale")}
            layout="wide"
            eyebrow="Why the scale is hard"
            title="Four billion [independent] parameters."
            kicker="No two organisms share weights, so each sweep of the brains streams the whole bank from memory. At 400,000 organisms, bandwidth runs out before arithmetic."
          >
            <Reveal>
              <ScaleCalculator />
            </Reveal>
          </Section>

          <Section
            id="status"
            index={index("evidence", "status")}
            eyebrow="Gates G0–G10"
            title="Honest [status]."
            kicker={
              <>
                Foundations built and tested; full scale, endurance and export not yet. 400,000 organisms at 30 ticks a
                second is a target.{" "}
                <a className={link} href={siteConfig.readme}>
                  The README
                </a>
                .
              </>
            }
          >
            <Reveal>
              <div className="panel rounded-[1.75rem] p-6 sm:p-8">
                <div className="flex items-center gap-2">
                  <MapIcon className="h-4 w-4 text-green-400" aria-hidden="true" />
                  <MicroLabel>Milestones: evidence gates, not dates</MicroLabel>
                </div>
                <ol className="mt-6 space-y-3">
                  {milestones.map((milestone) => (
                    <li
                      key={milestone.id}
                      className="grid grid-cols-[2.5rem_minmax(0,1fr)] items-start gap-x-3 gap-y-1.5 sm:grid-cols-[2.5rem_minmax(0,1fr)_auto]"
                    >
                      <span className="font-mono text-sm font-medium text-[color:var(--chapter)]">{milestone.id}</span>
                      <span className="min-w-0 text-sm">
                        <span className="font-bold text-white">{milestone.name}.</span>{" "}
                        <span className="text-slate-400">{milestone.deliverable}.</span>
                      </span>
                      <span className="col-start-2 flex flex-wrap gap-1 sm:col-start-3">
                        {milestone.gates.length === 0 ? (
                          <span className="rounded-md border border-white/10 px-1.5 py-0.5 font-mono text-[10px] text-slate-500">
                            per extension
                          </span>
                        ) : (
                          milestone.gates.map((id) => {
                            const gate = gateById.get(id);
                            const style = gate ? gateStyle[gate.state] : gateStyle.open;
                            return (
                              <span
                                key={id}
                                title={gate ? `${gate.name}: ${style.label}` : undefined}
                                className={cn("rounded-md border px-1.5 py-0.5 font-mono text-[10px] font-bold", style.className)}
                              >
                                {id}
                              </span>
                            );
                          })
                        )}
                      </span>
                    </li>
                  ))}
                </ol>
              </div>
            </Reveal>
            {/* Every gate as a tile, coloured by its state: the page's status at a glance. */}
            <Reveal className="mt-5">
              <ul className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
                {gates.map((gate) => {
                  const style = gateStyle[gate.state];
                  return (
                    <li key={gate.id} className={cn("flex flex-col rounded-2xl border p-4", style.className)}>
                      <div className="flex items-center justify-between gap-2">
                        <span className="font-mono text-sm font-bold">{gate.id}</span>
                        <span className="inline-flex items-center gap-1 font-mono text-[9.5px] tracking-[0.1em] uppercase">
                          <style.Icon className="h-3 w-3" aria-hidden="true" /> {style.label}
                        </span>
                      </div>
                      <h4 className="mt-2 text-[15px] leading-snug font-bold text-white">{gate.name}</h4>
                      <p className="mt-1 text-[12.5px] leading-snug text-slate-400">{gate.detail}</p>
                    </li>
                  );
                })}
              </ul>
            </Reveal>
          </Section>
        </ChapterBlock>

        {/* ─── 06 · Use it ────────────────────────────────────────────── */}
        <ChapterBlock chapter={chapter("lab")}>
          <Section
            id="worlds"
            index={index("lab", "worlds")}
            layout="wide"
            eyebrow="Presets"
            title="Ten [worlds] to open."
            kicker="Three operating profiles, five showcase worlds and the two v2 presets, each a file in presets/."
          >
            <ul className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-4">
              {presets.map((preset, at) => (
                <li key={preset.file}>
                  <Reveal className="panel lift flex h-full flex-col rounded-[1.5rem] p-5" delay={(at % 4) * 0.05}>
                    <code className="font-mono text-[13px] font-bold text-white">{preset.file}</code>
                    <span
                      className={cn(
                        "mt-2 self-start rounded-full border px-2 py-0.5 font-mono text-[10px] font-medium tracking-[0.08em] uppercase",
                        preset.kind === "profile"
                          ? "border-green-500/30 bg-green-500/10 text-green-300"
                          : preset.kind === "v2"
                            ? "border-cyan-400/30 bg-cyan-400/10 text-cyan-200"
                            : "border-amber-400/30 bg-amber-400/10 text-amber-200",
                      )}
                    >
                      {preset.chosen}
                    </span>
                    <p className="mt-3 font-mono text-[11px] text-slate-500">{preset.model}</p>
                    <PopulationBar founders={preset.founders} ceiling={preset.ceiling} />
                    <p className="mt-2 font-mono text-[11px] text-slate-400 tabular-nums">
                      {preset.grid}² field · {preset.torus}-unit torus · Δt {preset.dt} s
                    </p>
                    <p className="mt-3 text-sm leading-relaxed text-slate-400">{preset.body}</p>
                  </Reveal>
                </li>
              ))}
            </ul>
          </Section>

          <Section
            id="studio"
            index={index("lab", "studio")}
            layout="wide"
            eyebrow="Studio and laboratory"
            title="The Studio and the [command line]."
            kicker="Run, pause, step, zoom, inspect, brush and record in the Studio; headless runs, branches and experiments on the command line."
          >
            <div className="space-y-5">
              <Reveal>
                <StudioKeys />
              </Reveal>
              <Reveal>
                <div className="panel rounded-[1.75rem] p-6 sm:p-8">
                  <div className="flex items-center gap-2">
                    <Terminal className="h-4 w-4 text-green-400" aria-hidden="true" />
                    <MicroLabel>The flife command line</MicroLabel>
                  </div>
                  {/* slop-ok(S12-three-cards): a ruled list of every subcommand that wraps to three columns, not three cards. */}
                  <dl className="mt-6 grid grid-cols-1 gap-x-10 gap-y-3.5 md:grid-cols-2 xl:grid-cols-3">
                    {cliSurface.map((entry) => (
                      <div key={entry.name} className="flex gap-3 border-t border-white/[0.05] pt-3.5">
                        <dt className="w-28 shrink-0 font-mono text-sm text-green-300">{entry.name}</dt>
                        <dd className="text-sm leading-snug text-slate-400">{entry.body}</dd>
                      </div>
                    ))}
                  </dl>
                  <p className="mt-6 text-xs leading-relaxed text-slate-500">
                    Every command prints versioned NDJSON with typed errors.
                  </p>
                </div>
              </Reveal>
            </div>
          </Section>

          <Section
            id="stack"
            index={index("lab", "stack")}
            eyebrow="The FrankenSuite"
            title="Built on the [Franken] stack."
            kicker="FrankenLife builds on these five projects, and some of its work lands upstream in them."
          >
            <Reveal>
              <ul className="panel ruled overflow-hidden rounded-[1.75rem]">
                {stack.map((part) => (
                  <li key={part.name}>
                    <a
                      href={part.href}
                      className="group grid grid-cols-[minmax(0,1fr)_auto] items-start gap-x-4 gap-y-1 p-5 transition-colors hover:bg-green-500/[0.04] sm:grid-cols-[13rem_minmax(0,1fr)_auto] sm:p-6"
                    >
                      <span className="text-base font-extrabold text-white group-hover:text-green-200">{part.name}</span>
                      <ExternalLink className="h-4 w-4 text-slate-500 group-hover:text-green-300 sm:order-last" aria-hidden="true" />
                      <span className="col-span-2 text-sm leading-relaxed text-slate-400 sm:col-span-1">{part.role}</span>
                    </a>
                  </li>
                ))}
              </ul>
            </Reveal>
          </Section>

          <Section
            id="try"
            index={index("lab", "try")}
            eyebrow="Laboratory mode"
            title="Run it [yourself]."
            kicker="What your machine can run, the 4,096-organism reference world, its frames, and the Studio."
          >
            <div className="space-y-3">
              {commands.slice(0, FIRST_COMMANDS).map((entry) => (
                <Reveal key={entry.command}>
                  <CopyCommand comment={entry.comment} command={entry.command} />
                </Reveal>
              ))}
            </div>
            <details className="group mt-3 rounded-2xl border border-white/[0.08] bg-black/20">
              <summary className="flex min-h-12 cursor-pointer list-none items-center gap-2 px-5 text-sm font-semibold text-slate-300 hover:text-white [&::-webkit-details-marker]:hidden">
                <Terminal className="h-4 w-4 text-green-400" aria-hidden="true" />
                {commands.length - FIRST_COMMANDS} more: branches, v2 worlds, 4K films
                <span aria-hidden="true" className="ml-auto font-mono text-xs text-slate-500 group-open:rotate-45">
                  +
                </span>
              </summary>
              <div className="space-y-3 px-3 pb-3">
                {commands.slice(FIRST_COMMANDS).map((entry) => (
                  <CopyCommand key={entry.command} comment={entry.comment} command={entry.command} />
                ))}
              </div>
            </details>
          </Section>
        </ChapterBlock>
      </main>

      <footer data-outside className="dock-clearance relative isolate overflow-hidden border-t border-white/[0.06]">
        <div aria-hidden="true" className="field-light absolute inset-0 -z-10" />
        <div className="mx-auto max-w-7xl px-4 pt-24 pb-14 sm:px-6 md:pt-32">
          <div className="grid grid-cols-1 gap-12 lg:grid-cols-12">
            <div className="lg:col-span-7">
              <p className="display-2 font-extrabold text-white">
                <GreenTitle text="The code is [open]." />
              </p>
              <p className="mt-6 max-w-2xl text-sm leading-relaxed text-slate-400">
                <span className="font-semibold text-slate-300">Inspiration, not a port.</span> {inspiration}
              </p>
              <a
                href={siteConfig.github}
                className="mt-8 inline-flex min-h-12 items-center gap-2 rounded-2xl bg-green-500 px-5 text-[15px] font-extrabold text-black transition-colors hover:bg-green-400"
              >
                Open the repository <ArrowRight className="h-4 w-4" aria-hidden="true" />
              </a>
            </div>
            <nav aria-label="Project" className="lg:col-span-5">
              <ul className="grid grid-cols-1 gap-2.5 text-sm min-[420px]:grid-cols-2">
                <FooterLink href={siteConfig.github} label="Repository" />
                <FooterLink href={siteConfig.plan} label="The design plan" />
                <FooterLink href={siteConfig.fl048} label="Experiment record" />
                <FooterLink href={siteConfig.showcase} label="The showcase search" />
                <FooterLink href={siteConfig.agents} label="Rules for agents" />
                <FooterLink href={siteConfig.license} label="License (MIT + rider)" />
              </ul>
            </nav>
          </div>
          <p className="mt-16 flex flex-wrap items-center gap-x-6 gap-y-2 border-t border-white/[0.06] pt-6 font-mono text-[11px] text-slate-500">
            <span>{siteConfig.name}</span>
            <span>Every figure names its source</span>
          </p>
        </div>
      </footer>
    </>
  );
}

/** Founders and population ceiling on one logarithmic axis, 1,000 to 1,000,000. */
function PopulationBar({ founders, ceiling }: { founders: number; ceiling: number }) {
  const at = (n: number) => `${((Math.log10(n) - 3) / 3) * 100}%`;
  return (
    <div className="mt-4">
      <div
        className="relative h-2 rounded-full bg-white/[0.05]"
        role="img"
        aria-label={`${grouped(founders)} founders, a ceiling of ${grouped(ceiling)}`}
      >
        <span
          aria-hidden="true"
          className="absolute inset-y-0 rounded-full bg-gradient-to-r from-green-500/70 to-cyan-400/50"
          style={{ left: at(founders), width: `calc(${at(ceiling)} - ${at(founders)})` }}
        />
        <span
          aria-hidden="true"
          className="absolute top-1/2 h-3 w-3 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-[#020a02] bg-green-300"
          style={{ left: at(founders) }}
        />
        <span aria-hidden="true" className="absolute top-1/2 h-3 w-[2px] -translate-y-1/2 bg-cyan-200" style={{ left: at(ceiling) }} />
      </div>
      <div className="mt-1.5 flex justify-between font-mono text-[10px] text-slate-500 tabular-nums">
        <span>
          <span className="text-green-300">{grouped(founders)}</span> founders
        </span>
        <span>
          ceiling <span className="text-cyan-200">{grouped(ceiling)}</span>
        </span>
      </div>
    </div>
  );
}

function FooterLink({ href, label }: { href: string; label: string }) {
  return (
    <li>
      <a
        href={href}
        className="flex min-h-12 items-center justify-between gap-2 rounded-xl border border-white/[0.08] bg-white/[0.015] px-4 text-slate-300 transition-colors hover:border-green-400/40 hover:text-green-300"
      >
        {label} <ExternalLink className="h-3.5 w-3.5" aria-hidden="true" />
      </a>
    </li>
  );
}
