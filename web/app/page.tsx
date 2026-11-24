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
import { V2Explorer } from "@/components/explain/v2-explorer";
import { IndependentWeights } from "@/components/explain/weights";
import { ZoomLadder } from "@/components/explain/zoom-ladder";
import { Film } from "@/components/film";
import { GreenTitle, MicroLabel, Reticle } from "@/components/franken";
import { Reveal } from "@/components/reveal";
import { RevealController } from "@/components/reveal-controller";
import { SEED_TONES, SeedStrips } from "@/components/seed-strip";
import { ChapterBlock, Section } from "@/components/section";
import { SiteHeader } from "@/components/site-header";
import { SkipOffscreen } from "@/components/skip-offscreen";
import {
  branchObservation,
  type Chapter,
  chapters,
  claims,
  cliSurface,
  commands,
  composes,
  filmChoice,
  gates,
  type GateState,
  heroStats,
  hypotheses,
  inheritance,
  inspiration,
  milestones,
  notThis,
  presets,
  priorWork,
  shortcuts,
  siteConfig,
  stack,
  v2BuildStatus,
  v2VisualMappings,
  type Verdict,
  worldModel,
} from "@/lib/content";
import { film } from "@/lib/media";
import { cn, grouped } from "@/lib/utils";

const { world, render, video } = film;
const seconds = (ticks: number) => (ticks * world.timestep[0]) / world.timestep[1];

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

export default function Home() {
  const firstLive = world.live[0] ?? 0;
  const lastLive = world.live.at(-1) ?? 0;
  const lowest = Math.min(...world.live);
  const highest = Math.max(...world.live);
  const reproduce = [
    "# flife is `cargo run --release -p flife-cli --` in the repository",
    `flife run ${world.preset ?? "CONFIG"} --ticks ${world.from.tick} --run-dir RUN`,
    render.command.replace("FRAMES", "frames"),
    ...(render.camera ? ["# CAMERA: lib/media.json's render.camera keys, as {\"keys\": [...]}"] : []),
  ];
  const crfAt = video.encode.indexOf("-crf");
  const crf = crfAt >= 0 ? video.encode[crfAt + 1] : undefined;

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
              <Film feather className="mx-auto w-full lg:max-w-[min(100%,calc((100svh-8rem)*var(--film-aspect)))]" />
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
                A real-time artificial-life laboratory. Every organism in this film carries its own heritable neural
                network; they feed, signal, move, split and die through one shared field of energy and chemistry,
                and nothing is scripted or trained. Pause it, inspect it, branch it, replay it: the same world, to
                the last bit, every time.
              </p>
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
                . Every figure on this page names where it comes from.
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
            kicker={
              <>
                Each organism is a small neural network with a body, a metabolism and offspring, living in one shared
                field of energy and chemistry. Nothing is scripted and nothing is trained: whatever lineages survive,
                they survive by these rules alone.
              </>
            }
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
            kicker={
              <>
                Before the real rules, a toy written for this page: about a thousand organisms, each with its own tiny
                network, under four of FrankenLife&apos;s rules in miniature. Select one to watch its brain, brush
                energy into the field, then replay from the seed and land on the same digest.
              </>
            }
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
            kicker="Each of these shortcuts can produce a gorgeous video of nothing evolving. FrankenLife's rules forbid all four, and its tests hold it to them."
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

            <Reveal className="mt-5">
              <div className="panel relative rounded-[1.75rem] p-6 sm:p-8">
                <Reticle inset={9} size={12} />
                <div className="grid grid-cols-1 gap-9 lg:grid-cols-2">
                  <div>
                    <MicroLabel className="text-slate-400">What already exists</MicroLabel>
                    <ul className="mt-5 space-y-3.5">
                      {priorWork.map((entry) => (
                        <li key={entry.name} className="text-sm leading-relaxed text-slate-400">
                          <span className="font-bold text-slate-200">{entry.name}</span> {entry.shows}.
                        </li>
                      ))}
                    </ul>
                  </div>
                  <div>
                    <MicroLabel>What FrankenLife composes</MicroLabel>
                    <ol className="mt-5 space-y-3.5">
                      {composes.map((entry, at) => (
                        <li key={entry.title} className="flex gap-3">
                          <span className="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-lg border border-green-500/30 bg-green-500/10 font-mono text-xs font-bold text-green-300">
                            {at + 1}
                          </span>
                          <span className="text-sm leading-relaxed">
                            <span className="font-bold text-white">{entry.title}.</span>{" "}
                            <span className="text-slate-400">{entry.body}</span>
                          </span>
                        </li>
                      ))}
                    </ol>
                  </div>
                </div>
                <p className="mt-7 border-t border-white/[0.06] pt-5 text-sm leading-relaxed text-slate-400">
                  <span className="font-bold text-amber-200">Target, not a result:</span> all four at the scale of the
                  reference screenshots, about 400,000 organisms and four billion independent parameters, where memory
                  bandwidth rather than arithmetic is the limit.{" "}
                  <a className={link} href="#scale">
                    Why that is hard
                  </a>
                  .
                </p>
              </div>
            </Reveal>

            <Reveal className="mt-5">
              <div className="rounded-[1.75rem] border border-dashed border-white/[0.1] p-6 sm:p-7">
                <MicroLabel className="text-slate-400">What it is not</MicroLabel>
                <ul className="mt-4 grid grid-cols-1 gap-x-8 gap-y-2.5 text-sm leading-relaxed text-slate-400 sm:grid-cols-2">
                  {notThis.map((line) => (
                    <li key={line} className="flex gap-2.5">
                      <span aria-hidden="true" className="mt-2 h-1 w-1 shrink-0 rounded-full bg-slate-500" />
                      {line}
                    </li>
                  ))}
                </ul>
              </div>
            </Reveal>
          </Section>

          <Section
            id="controls"
            index={index("different", "controls")}
            layout="wide"
            eyebrow="The model and its controls"
            title="Remove a cause, lose the [structure]."
            kicker={
              <>
                The film&apos;s world, run again twice with one rule changed. Without emission nothing ever leaves an
                organism, and the tissue becomes a haze; with fresh offspring instead of inherited ones, the world
                dwindles to scatter. Same preset, seed, founders and food.
              </>
            }
          >
            <Reveal>
              <ControlsTrio />
            </Reveal>
          </Section>

          <Section
            id="film"
            index={index("different", "film")}
            eyebrow="Provenance"
            title="What you are [watching]."
            kicker={
              <>
                Not an illustration: <code className="text-green-300">flife render</code> drew each frame from the
                committed world at its tick. The world is deterministic: the same commands reach the same digests,
                below, on any machine.
              </>
            }
          >
            <div className="grid grid-cols-1 gap-5 md:grid-cols-2">
              <Reveal>
                <div className="panel relative h-full rounded-[1.75rem] p-6 sm:p-8">
                  <Reticle inset={9} size={12} />
                  <MicroLabel>The world</MicroLabel>
                  <dl className="ruled mt-5 text-sm">
                    <Row label="Preset" value={world.preset ?? world.name} mono />
                    {world.model ? <Row label="Model" value={world.model} mono /> : null}
                    <Row label="Seed" value={String(world.seed)} mono />
                    <Row label="Founders" value={`${grouped(world.founders)} on a ${world.grid[0]} × ${world.grid[1]} field`} />
                    <Row label="Tick length" value={`${world.timestep[0]}/${world.timestep[1]} s of simulated time`} />
                    <Row
                      label="Evolved first"
                      value={`${grouped(world.from.tick)} ticks (${seconds(world.from.tick).toFixed(0)} simulated seconds)`}
                    />
                    <Row
                      label="Alive in the film"
                      value={lowest === highest ? grouped(lowest) : `${grouped(lowest)} – ${grouped(highest)}`}
                    />
                  </dl>
                </div>
              </Reveal>
              <Reveal delay={0.08}>
                <div className="panel relative h-full rounded-[1.75rem] p-6 sm:p-8">
                  <Reticle inset={9} size={12} />
                  <MicroLabel>The film</MicroLabel>
                  <dl className="ruled mt-5 text-sm">
                    <Row label="Ticks" value={`${grouped(world.from.tick)} → ${grouped(world.to.tick)}, a frame every ${world.every}`} />
                    <Row label="Frames" value={`${video.frames} at ${video.fps} fps, ${video.width} × ${video.height}`} />
                    {video.segments && video.segments.length > 1 ? (
                      <Row
                        label="Segments"
                        value={`${video.segments.length} × ≤25 MiB (${(
                          video.segments.reduce((sum, seg) => sum + seg.bytes, 0) /
                          (1024 * 1024)
                        ).toFixed(1)} MiB total, preloaded sequentially)`}
                      />
                    ) : null}
                    <Row label="Colour" value={render.meaning} />
                    <Row
                      label="Field"
                      value={`${render.field_mapping === "none" ? "not drawn" : render.field_mapping}, ${render.look} look`}
                    />
                    <Row
                      label="Camera"
                      value={render.camera ? `${render.camera.keys.length} keys, chosen from the run` : "the whole torus, still"}
                    />
                    {render.features && render.features.length > 0 ? (
                      <Row label="V2 toggles" value={render.features.join(" ")} mono />
                    ) : null}
                    <Row label="First digest" value={world.from.digest} mono />
                    <Row label="Last digest" value={world.to.digest} mono />
                  </dl>
                </div>
              </Reveal>
            </div>
            <Reveal className="mt-5">
              <div className="panel rounded-[1.75rem] p-6 sm:p-8">
                <MicroLabel>How this world was chosen</MicroLabel>
                <div className="mt-4 space-y-3 text-[15px] leading-relaxed text-slate-300">
                  {filmChoice.world.map((paragraph) => (
                    <p key={paragraph.slice(0, 24)}>{paragraph}</p>
                  ))}
                  {render.camera ? <p>{filmChoice.camera}</p> : null}
                  <p>
                    <a className={link} href={siteConfig.showcase}>
                      The search, every sample&apos;s score and the held-out seeds
                    </a>
                    .
                  </p>
                </div>
              </div>
            </Reveal>
            <Reveal className="mt-5">
              <div className="panel rounded-[1.75rem] p-6 sm:p-8">
                <div className="flex items-center gap-2">
                  <Terminal className="h-4 w-4 text-green-400" aria-hidden="true" />
                  <MicroLabel>Make it yourself</MicroLabel>
                </div>
                <pre className="mt-4 overflow-x-auto rounded-xl border border-white/[0.06] bg-black/40 p-4 text-[13px] leading-relaxed text-green-200">
                  <code>{reproduce.join("\n")}</code>
                </pre>
                <p className="mt-4 text-xs leading-relaxed text-slate-400">
                  Rendered at revision <code className="text-slate-300">{render.revision.slice(0, 12)}</code> with
                  controllers on {render.controllers}; no result depends on the route. The first frame shows{" "}
                  {grouped(firstLive)} organisms and the last {grouped(lastLive)}; the render&apos;s{" "}
                  <code className="text-slate-300">end</code> record names the last digest above. The PNG frames are
                  flife&apos;s; this page&apos;s video encodes them with flife&apos;s own BT.709 colour chain at CRF{" "}
                  {crf} for the web
                  {video.segments && video.segments.length > 1
                    ? ` across ${video.segments.length} preloaded segments (each ≤25 MiB, first SHA-256 `
                    : ", and its SHA-256 is "}
                  <code className="break-all text-slate-300">{video.sha256}</code>
                  {video.segments && video.segments.length > 1 ? ")." : "."}
                </p>
              </div>
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
            kicker="Each rule has a plausible-looking wrong version, and the plan pins each one with a known answer small enough to compute by hand. Change the numbers: the page recomputes the rule beside its wrong twin."
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

          <Section
            id="v2"
            index={index("machinery", "v2")}
            layout="wide"
            eyebrow="field-ecology-v2 · M6 extension"
            title="Modular [biology], toggled clean."
            kicker={
              <>
                Every <code className="text-green-300">field-ecology-v1*</code> run omits <code className="text-slate-200">[v2]</code>{" "}
                and stays byte-identical. On <code className="text-green-300">field-ecology-v2</code>, 21 mechanisms are registered as
                individually toggleable slugs grouped into two Stage-2 verified bundles: Bundle A (<code className="text-slate-200">tissue</code>,
                14 active features, <code className="text-slate-200">[contact]</code>) and Bundle B (<code className="text-slate-200">supercolony</code>,
                11 active features, strictly <code className="text-slate-200">INV-08</code> pure-field). Every override materializes before
                hashing and is locked at founding time (<code className="text-slate-200">INV-14</code>).
              </>
            }
          >
            <div className="space-y-5">
              <Reveal>
                <V2Explorer />
              </Reveal>

              <Reveal>
                <div className="panel rounded-[1.75rem] p-6 sm:p-8">
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <MicroLabel>V2-16 · pure read-only visual mappings (INV-09)</MicroLabel>
                    <span className="rounded-full border border-cyan-400/30 bg-cyan-400/10 px-2.5 py-0.5 font-mono text-[10px] font-medium tracking-[0.1em] text-cyan-200 uppercase">
                      Zero overhead on v1
                    </span>
                  </div>
                  <dl className="mt-5 grid grid-cols-1 gap-4 md:grid-cols-2">
                    {v2VisualMappings.map((mapping) => (
                      <div key={mapping.name} className="rounded-2xl border border-white/[0.06] bg-black/30 p-4">
                        <div className="flex flex-wrap items-center justify-between gap-2">
                          <dt className="font-mono text-sm font-bold text-white">{mapping.name}</dt>
                          <span className="font-mono text-[10px] text-cyan-200">
                            {mapping.kind} · Studio key {mapping.key}
                          </span>
                        </div>
                        <dd className="mt-2 text-sm leading-relaxed text-slate-400">{mapping.body}</dd>
                        <dd className="mt-2 font-mono text-[11px] text-green-300/90">{mapping.flag}</dd>
                      </div>
                    ))}
                  </dl>
                  <p className="mt-5 text-xs leading-relaxed text-slate-500">
                    All four overlays read optional epoch slices and never alter simulation state (INV-09). Absent planes
                    degrade cleanly without heap allocation, and v1 runs ignore keys 8 and 9. See{" "}
                    <a className={link} href="#status">
                      Honest status
                    </a>{" "}
                    for the 104-run Stage-2 V2-17 empirical results and explicit residual tradeoffs.
                  </p>
                </div>
              </Reveal>
            </div>
          </Section>
        </ChapterBlock>

        {/* ─── 04 · What it can show ──────────────────────────────────── */}
        <ChapterBlock chapter={chapter("evidence")}>
          <Section
            id="replay"
            index={index("evidence", "replay")}
            layout="wide"
            eyebrow="Checkpoints · resume · branches"
            title="Rewind, [branch], compare."
            kicker="A kept run checkpoints as it goes. Kill it and resume, and it reaches the uninterrupted world. Branch it, and a control reaches the parent's world exactly, while an intervention differs only from its brush onwards. That is what makes an experiment a comparison."
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
                    The reference preset kept for 512 ticks and stepped to {grouped(branchObservation.from)}, then
                    branched twice from that checkpoint: a control, and a branch with one energy brush (
                    {branchObservation.brush}). All three resumed to tick {grouped(branchObservation.tick)}:
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
                  <p className="mt-5 text-xs leading-relaxed text-slate-500">
                    Each line is the run&apos;s <code className="text-slate-400">end</code> record&apos;s SHA-256 world
                    digest, on an {branchObservation.host}, with a release <code className="text-slate-400">flife</code>{" "}
                    (SHA-256 {branchObservation.binary}…). The commands are under{" "}
                    <a className={link} href="#try">
                      Run it yourself
                    </a>
                    ; the repository&apos;s own tests make the same comparison on every change.
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
                Each hypothesis had its rule written down before the runs, discovery and held-out seeds kept apart, and
                failed runs kept. One control came out strongly; two hypotheses are not supported, and the rest are
                stated as narrowly as the data allow.{" "}
                <a className={link} href={siteConfig.fl048}>
                  The full record
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
            id="claims"
            index={index("evidence", "claims")}
            eyebrow="Four claims"
            title="Kept [apart]."
            kicker="A beautiful image establishes only the visual claim. A speed measured on named hardware is a separate kind of result, and evidence for none of the four."
          >
            <Reveal>
              <ol className="panel grid grid-cols-1 overflow-hidden rounded-[1.75rem] sm:grid-cols-2">
                {claims.map((claim, at) => (
                  <li
                    key={claim.claim}
                    className={cn(
                      "border-white/[0.06] p-6 sm:p-7",
                      at > 0 && "border-t",
                      at === 1 && "sm:border-t-0",
                      at % 2 === 1 && "sm:border-l",
                    )}
                  >
                    <p className="flex items-baseline gap-3">
                      <span className="font-mono text-xs text-slate-500 tabular-nums">0{at + 1}</span>
                      <MicroLabel className="text-[color:var(--chapter)]">{claim.claim}</MicroLabel>
                    </p>
                    <h4 className="mt-3 text-lg text-white">{claim.meaning}</h4>
                    <p className="mt-2 text-sm leading-relaxed text-slate-400">{claim.evidence}</p>
                  </li>
                ))}
              </ol>
            </Reveal>
          </Section>

          <Section
            id="scale"
            index={index("evidence", "scale")}
            layout="wide"
            eyebrow="Why the scale is hard"
            title="Four billion [independent] parameters."
            kicker="No two organisms share weights, so a full sweep cannot reuse them: it streams the whole bank from memory. At the target scale, bandwidth, not arithmetic, is the limit, and only a real measurement with the real layout can justify a real-time claim."
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
                The foundations are built and tested; the visual, full-scale rate, endurance and export gates are not.
                The 400,000-organism world at 30 ticks a second is a target, not a result.{" "}
                <a className={link} href={siteConfig.readme}>
                  The README keeps the full account
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
            <Reveal className="mt-5">
              <ul className="panel ruled overflow-hidden rounded-[1.75rem]">
                {gates.map((gate) => {
                  const style = gateStyle[gate.state];
                  return (
                    <li
                      key={gate.id}
                      className="grid grid-cols-[3rem_minmax(0,1fr)] gap-x-4 gap-y-2 p-5 sm:grid-cols-[3rem_minmax(0,1fr)_auto] sm:p-6"
                    >
                      <span className="font-mono text-sm font-medium text-[color:var(--chapter)]">{gate.id}</span>
                      <div className="min-w-0">
                        <h4 className="text-base text-white">{gate.name}</h4>
                        <p className="mt-1 text-sm leading-relaxed text-slate-400">{gate.detail}</p>
                      </div>
                      <span
                        className={cn(
                          "col-start-2 inline-flex h-fit items-center gap-1.5 justify-self-start rounded-full border px-2.5 py-0.5 font-mono text-[10px] font-medium tracking-[0.1em] whitespace-nowrap uppercase sm:col-start-3",
                          style.className,
                        )}
                      >
                        <style.Icon className="h-3 w-3" aria-hidden="true" /> {style.label}
                      </span>
                    </li>
                  );
                })}
              </ul>
            </Reveal>
            <Reveal className="mt-5">
              <div className="panel rounded-[1.75rem] p-6 sm:p-8">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <MicroLabel>field-ecology-v2 · branch status</MicroLabel>
                  <span className="rounded-full border border-white/15 bg-white/5 px-2.5 py-0.5 font-mono text-[10px] font-medium tracking-[0.1em] text-slate-300 uppercase">
                    Observed · Four Claims kept apart
                  </span>
                </div>
                <ul className="ruled mt-5">
                  {v2BuildStatus.map((item) => {
                    const style = gateStyle[item.tone];
                    return (
                      <li
                        key={item.stage}
                        className="grid grid-cols-1 gap-x-4 gap-y-2 py-4 first:pt-0 last:pb-0 sm:grid-cols-[11rem_minmax(0,1fr)_auto]"
                      >
                        <div>
                          <p className="text-sm font-bold text-white">{item.stage}</p>
                          <p className="mt-0.5 font-mono text-[10px] text-slate-500 uppercase">{item.claim}</p>
                        </div>
                        <p className="text-sm leading-relaxed text-slate-400">{item.detail}</p>
                        <span
                          className={cn(
                            "inline-flex h-fit items-center gap-1.5 justify-self-start rounded-full border px-2.5 py-0.5 font-mono text-[10px] font-medium tracking-[0.1em] whitespace-nowrap uppercase sm:justify-self-end",
                            style.className,
                          )}
                        >
                          <style.Icon className="h-3 w-3" aria-hidden="true" /> {style.label}
                        </span>
                      </li>
                    );
                  })}
                </ul>
              </div>
            </Reveal>
          </Section>
        </ChapterBlock>

        {/* ─── 05 · Use it ────────────────────────────────────────────── */}
        <ChapterBlock chapter={chapter("lab")}>
          <Section
            id="worlds"
            index={index("lab", "worlds")}
            layout="wide"
            eyebrow="Presets"
            title="Ten [worlds] to open."
            kicker={
              <>
                Three are the operating profiles, each sized by a declared experiment. Five were chosen by an outer
                search against image statistics of the reference screenshots and then by eye; the organisms inside them
                have no aesthetic fitness, and{" "}
                <a className={link} href={siteConfig.showcase}>
                  the search states how
                </a>
                . Two are Stage-2 verified <code className="text-slate-200">field-ecology-v2</code> presets (Bundles A and B,
                104 runs across discovery and held-out seeds with explicit residual tradeoffs).
              </>
            }
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
            title="Watch it, [touch] it, measure it."
            kicker="The native Studio is the microscope: run, pause, step, zoom, select, inspect, brush and record, while the session commits every tick. The flife command line is the laboratory around it."
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
                    Every command prints versioned NDJSON with stable identifiers and typed errors: a missing
                    accelerator, an exhausted budget, a schema mismatch, an incompatible checkpoint and a numerical fault
                    are each their own kind. Founding commands (<code className="text-slate-400">validate</code>,{" "}
                    <code className="text-slate-400">capabilities --config</code>, <code className="text-slate-400">run</code>,{" "}
                    <code className="text-slate-400">render CONFIG</code>, <code className="text-slate-400">experiment</code>,{" "}
                    <code className="text-slate-400">transplant</code>) accept ordered{" "}
                    <code className="text-slate-400">--disable</code>, <code className="text-slate-400">--enable</code> and{" "}
                    <code className="text-slate-400">--v1-compat</code> overrides;{" "}
                    <code className="text-slate-400">resume</code>, <code className="text-slate-400">step</code> and{" "}
                    <code className="text-slate-400">render --run-dir</code> refuse them (<code className="text-slate-400">INV-14</code>).
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
            kicker="FrankenLife composes the suite rather than re-implementing it, and some of its work lands upstream."
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
            kicker="Every command prints versioned NDJSON. Start with what your machine can run, then the 4,096-organism reference world, then branch it and compare."
          >
            <div className="space-y-3">
              {commands.map((entry) => (
                <Reveal key={entry.command}>
                  <CopyCommand comment={entry.comment} command={entry.command} />
                </Reveal>
              ))}
            </div>
          </Section>
        </ChapterBlock>
      </main>

      <footer data-outside className="dock-clearance relative isolate overflow-hidden border-t border-white/[0.06]">
        <div aria-hidden="true" className="field-light absolute inset-0 -z-10" />
        <div className="mx-auto max-w-7xl px-4 pt-24 pb-14 sm:px-6 md:pt-32">
          <div className="grid grid-cols-1 gap-12 lg:grid-cols-12">
            <div className="lg:col-span-7">
              <p className="display-2 font-extrabold text-white">
                <GreenTitle text="The world is [alive]." />
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
            <span>Every figure on this page names its source</span>
            <span>Recordings stay out of git; their manifests do not</span>
          </p>
        </div>
      </footer>
    </>
  );
}

function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex flex-col gap-0.5 py-2.5 sm:flex-row sm:items-baseline sm:justify-between sm:gap-6">
      <dt className="shrink-0 text-xs text-slate-500">{label}</dt>
      <dd className={cn("min-w-0 text-slate-200 sm:text-right", mono && "font-mono text-[13px] break-all")}>{value}</dd>
    </div>
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
