import {
  Atom,
  Brain,
  CircleCheck,
  CircleDashed,
  CircleDot,
  Clapperboard,
  ExternalLink,
  FlaskConical,
  Gauge,
  Layers,
  Microscope,
  Scale,
  Terminal,
  Workflow,
} from "lucide-react";

import { CopyCommand } from "@/components/copy-command";
import { Film } from "@/components/film";
import { FrankenContainer, FrankenStitch, GreenTitle, MicroLabel } from "@/components/franken";
import { Reveal } from "@/components/reveal";
import { RevealController } from "@/components/reveal-controller";
import { Section } from "@/components/section";
import { SiteHeader } from "@/components/site-header";
import {
  claims,
  commands,
  filmChoice,
  gates,
  type GateState,
  heroStats,
  hypotheses,
  inspiration,
  principles,
  siteConfig,
  stack,
  tickPhases,
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

function Glow() {
  return (
    <div aria-hidden="true" className="pointer-events-none absolute inset-0 -z-10 overflow-hidden">
      <div className="orbit top-[-10%] left-[-10%] h-[38rem] w-[38rem] bg-green-500" />
      <div className="orbit top-[30%] right-[-15%] h-[30rem] w-[30rem] bg-cyan-500 [animation-delay:-9s]" />
      <div className="orbit bottom-[-20%] left-[25%] h-[34rem] w-[34rem] bg-fuchsia-600 [animation-delay:-17s]" />
    </div>
  );
}

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
  const crf = video.encode[video.encode.indexOf("-crf") + 1];

  return (
    <>
      <SiteHeader />
      <RevealController />
      <main id="top" className="relative isolate">
        {/* Hero */}
        <section className="relative isolate overflow-hidden">
          <Glow />
          {/* Phones read the intro, the film, then the figures; wide screens put the film beside both. */}
          <div className="mx-auto grid max-w-7xl grid-cols-1 gap-10 px-4 pt-28 pb-16 sm:px-6 md:pt-36 lg:grid-cols-12 lg:gap-x-16 lg:gap-y-8 lg:pb-24">
            {/* A CSS entrance, not <Reveal>: the hero and the film show without JavaScript. */}
            <div className="rise-in space-y-8 lg:col-span-6 lg:self-end">
              <div className="inline-flex items-center gap-3">
                <span className="h-px w-8 bg-green-500/40" />
                <MicroLabel>Artificial life · Rust · Bit-exact</MicroLabel>
              </div>
              <h1 className="text-5xl text-white sm:text-6xl xl:text-7xl">
                <GreenTitle text="Evolution, [bit for bit]." />
              </h1>
              <p className="max-w-xl text-lg leading-relaxed text-slate-300">
                A real-time artificial-life laboratory in memory-safe Rust. Every organism carries its own heritable
                neural network and chemistry, and they feed, signal, move, split and die through one shared energy
                field. Watch, pause, inspect, branch and replay them: the same world, to the last bit, every time.
              </p>
              <div className="flex flex-wrap gap-3">
                <a
                  href="#film"
                  className="inline-flex items-center gap-2 rounded-2xl bg-green-500 px-5 py-3 text-sm font-black text-black transition hover:bg-green-400 focus-visible:ring-2 focus-visible:ring-green-300 focus-visible:outline-none"
                >
                  <Clapperboard className="h-4 w-4" aria-hidden="true" /> What you are watching
                </a>
                <a
                  href={siteConfig.github}
                  className="inline-flex items-center gap-2 rounded-2xl border border-white/15 px-5 py-3 text-sm font-bold text-white transition hover:border-green-400/50 hover:text-green-300 focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
                >
                  Read the code <ExternalLink className="h-4 w-4" aria-hidden="true" />
                </a>
              </div>
            </div>
            <div className="rise-in [--rise-delay:150ms] lg:col-span-6 lg:col-start-7 lg:row-span-2 lg:row-start-1 lg:self-center">
              <Film />
            </div>
            <ul className="rise-in grid grid-cols-2 gap-3 [--rise-delay:300ms] sm:grid-cols-4 lg:col-span-6 lg:col-start-1 lg:grid-cols-2 lg:self-start xl:grid-cols-4">
              {heroStats.map((stat) => (
                <li key={stat.label} className="glass-modern rounded-2xl p-4">
                  <p className="text-xl font-black text-white tabular-nums">{stat.value}</p>
                  <p className="mt-1 text-[10px] font-black tracking-[0.18em] text-green-500/80 uppercase">
                    {stat.label}
                  </p>
                  <p className="mt-2 text-xs leading-snug text-slate-400">{stat.note}</p>
                </li>
              ))}
            </ul>
          </div>
        </section>

        <FrankenStitch className="mx-auto max-w-5xl" />

        {/* The film's provenance */}
        <Section
          id="film"
          icon={Clapperboard}
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
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
            <Reveal>
              <FrankenContainer className="h-full p-6 sm:p-8">
                <MicroLabel>The world</MicroLabel>
                <dl className="mt-5 space-y-3 text-sm">
                  <Row label="Preset" value={world.preset ?? world.name} mono />
                  <Row label="Seed" value={String(world.seed)} mono />
                  <Row label="Founders" value={`${grouped(world.founders)} on a ${world.grid[0]} × ${world.grid[1]} field`} />
                  <Row label="Tick length" value={`${world.timestep[0]}/${world.timestep[1]} s of simulated time`} />
                  <Row label="Evolved first" value={`${grouped(world.from.tick)} ticks (${seconds(world.from.tick).toFixed(0)} simulated seconds)`} />
                  <Row label="Alive in the film" value={`${grouped(lowest)} – ${grouped(highest)}`} />
                </dl>
              </FrankenContainer>
            </Reveal>
            <Reveal delay={0.1}>
              <FrankenContainer className="h-full p-6 sm:p-8">
                <MicroLabel>The film</MicroLabel>
                <dl className="mt-5 space-y-3 text-sm">
                  <Row label="Ticks" value={`${grouped(world.from.tick)} → ${grouped(world.to.tick)}, a frame every ${world.every}`} />
                  <Row label="Frames" value={`${video.frames} at ${video.fps} fps, ${video.width} × ${video.height}`} />
                  <Row label="Colour" value={render.meaning} />
                  <Row label="Field" value={`${render.field_mapping}, ${render.look} look`} />
                  <Row
                    label="Camera"
                    value={
                      render.camera
                        ? `${render.camera.keys.length} keys, chosen from the run`
                        : "the whole torus, still"
                    }
                  />
                  <Row label="First digest" value={world.from.digest} mono />
                  <Row label="Last digest" value={world.to.digest} mono />
                </dl>
              </FrankenContainer>
            </Reveal>
          </div>
          <Reveal className="mt-4">
            <div className="glass-modern rounded-2xl p-5 sm:p-6">
              <MicroLabel>How this world was chosen</MicroLabel>
              <div className="mt-4 space-y-3 text-sm leading-relaxed text-slate-300">
                {filmChoice.world.map((paragraph) => (
                  <p key={paragraph.slice(0, 24)}>{paragraph}</p>
                ))}
                {render.camera ? <p>{filmChoice.camera}</p> : null}
                <p>
                  <a className="text-green-300 underline-offset-4 hover:underline" href={siteConfig.showcase}>
                    The search, every sample&apos;s score and the held-out seeds
                  </a>
                  .
                </p>
              </div>
            </div>
          </Reveal>
          <Reveal className="mt-4">
            <div className="glass-modern rounded-2xl p-5 sm:p-6">
              <MicroLabel>Make it yourself</MicroLabel>
              <pre className="mt-4 overflow-x-auto text-[13px] leading-relaxed text-green-200">
                <code>{reproduce.join("\n")}</code>
              </pre>
              <p className="mt-4 text-xs leading-relaxed text-slate-400">
                Rendered at revision <code className="text-slate-300">{render.revision.slice(0, 12)}</code> with
                controllers on {render.controllers}; no result depends on the route. The first frame shows{" "}
                {grouped(firstLive)} organisms and the last {grouped(lastLive)}; the render&apos;s{" "}
                <code className="text-slate-300">end</code> record names the last digest above. The PNG frames are
                flife&apos;s; this page&apos;s video encodes them with flife&apos;s own BT.709 colour chain at CRF {crf}{" "}
                for the web, and its SHA-256 is <code className="break-all text-slate-300">{video.sha256}</code>.
              </p>
            </div>
          </Reveal>
        </Section>

        {/* Principles */}
        <Section
          icon={Brain}
          eyebrow="The rules"
          title="Every organism, its [own] brain."
          kicker="None of the shapes in the film is painted. They have to come out of an ecology with finite energy, genuine inheritance and a ledger that balances on every tick."
        >
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            {principles.map((principle, index) => (
              <Reveal key={principle.title} delay={index * 0.05}>
                <div className="kinetic-card glass-modern h-full rounded-2xl p-6">
                  <h3 className="text-lg text-white">{principle.title}</h3>
                  <p className="mt-2 text-sm leading-relaxed text-slate-400">{principle.body}</p>
                </div>
              </Reveal>
            ))}
          </div>
        </Section>

        {/* The model */}
        <Section
          id="world"
          icon={Atom}
          eyebrow="FieldEcologyV1"
          title="How the world [works]."
          kicker="An original, explicit rule set: a periodic field of energy and chemistry, and organisms that can only touch each other through it."
        >
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
            {worldModel.map((part, index) => (
              <Reveal key={part.name} delay={(index % 3) * 0.05}>
                <div className="kinetic-card glass-modern h-full rounded-2xl p-5">
                  <MicroLabel>{part.name}</MicroLabel>
                  <p className="mt-3 text-sm leading-relaxed text-slate-300">{part.body}</p>
                </div>
              </Reveal>
            ))}
          </div>
          <Reveal className="mt-4">
            <FrankenContainer className="p-6 sm:p-8">
              <MicroLabel>The ledger</MicroLabel>
              <p className="mt-4 overflow-x-auto font-mono text-sm whitespace-nowrap text-green-200 sm:text-base">
                𝓔ₙ₊₁ − 𝓔ₙ = injection − removal − dissipation + numerical residual
              </p>
              <p className="mt-3 text-sm leading-relaxed text-slate-400">
                𝓔 is the energy in every organism plus the energy in every cell. Emission, absorption and a
                child&apos;s endowment only move it. No correction may add energy to make the equation balance.
              </p>
            </FrankenContainer>
          </Reveal>
        </Section>

        {/* One tick */}
        <Section
          id="tick"
          icon={Workflow}
          eyebrow="Ten phases"
          title="One [tick]."
          kicker="Each tick is prepared as a candidate against the committed world and published whole, or not at all. Cancelling before phase 10 discards it; the last committed world stays authoritative."
        >
          <ol className="space-y-2">
            {tickPhases.map((phase, index) => (
              <li key={phase}>
                <Reveal className="glass-modern flex items-start gap-4 rounded-2xl px-5 py-4" delay={index * 0.03}>
                  <span className="w-7 shrink-0 font-mono text-sm font-black text-green-400 tabular-nums">
                    {String(index + 1).padStart(2, "0")}
                  </span>
                  <span className="text-sm leading-relaxed text-slate-200">{phase}</span>
                </Reveal>
              </li>
            ))}
          </ol>
        </Section>

        {/* Four claims */}
        <Section
          icon={Scale}
          eyebrow="Four claims"
          title="Kept [apart]."
          kicker="A beautiful image establishes only the visual claim. A speed measured on named hardware is a separate kind of result, and evidence for none of the four."
        >
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            {claims.map((claim, index) => (
              <Reveal key={claim.claim} delay={index * 0.05}>
                <div className="glass-modern h-full rounded-2xl p-6">
                  <MicroLabel>{claim.claim}</MicroLabel>
                  <h3 className="mt-3 text-lg text-white">{claim.meaning}</h3>
                  <p className="mt-2 text-sm leading-relaxed text-slate-400">{claim.evidence}</p>
                </div>
              </Reveal>
            ))}
          </div>
        </Section>

        {/* Status */}
        <Section
          id="status"
          icon={Gauge}
          eyebrow="Gates G0–G10"
          title="Honest [status]."
          kicker={
            <>
              The foundations are built and tested; the visual, full-scale rate, endurance and export gates are not. The 400,000
              organism world at 30 ticks a second is a target, not a result.{" "}
              <a className="text-green-300 underline-offset-4 hover:underline" href={siteConfig.readme}>
                The README keeps the full account.
              </a>
            </>
          }
        >
          <ul className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            {gates.map((gate, index) => {
              const style = gateStyle[gate.state];
              return (
                <li key={gate.id}>
                  <Reveal className="glass-modern h-full rounded-2xl p-5" delay={(index % 2) * 0.05}>
                    <div className="flex items-center justify-between gap-3">
                      <span className="font-mono text-sm font-black text-green-400">{gate.id}</span>
                      <span
                        className={cn(
                          "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-[10px] font-black tracking-[0.12em] uppercase",
                          style.className,
                        )}
                      >
                        <style.Icon className="h-3 w-3" aria-hidden="true" /> {style.label}
                      </span>
                    </div>
                    <h3 className="mt-3 text-base text-white">{gate.name}</h3>
                    <p className="mt-1.5 text-sm leading-relaxed text-slate-400">{gate.detail}</p>
                  </Reveal>
                </li>
              );
            })}
          </ul>
        </Section>

        {/* Science */}
        <Section
          id="science"
          icon={FlaskConical}
          eyebrow="FL-048 · declared before running"
          title="What the experiments [found]."
          kicker={
            <>
              Five hypotheses, each with its rule written down before the runs, discovery and held-out seeds kept
              apart, and failed runs kept. Two are not supported, and the rest are stated as narrowly as the data
              allow.{" "}
              <a className="text-green-300 underline-offset-4 hover:underline" href={siteConfig.fl048}>
                The full record.
              </a>
            </>
          }
        >
          <div className="space-y-3">
            {hypotheses.map((hypothesis, index) => (
              <Reveal key={hypothesis.id} delay={index * 0.04}>
                <div className="glass-modern rounded-2xl p-5 sm:p-6">
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <h3 className="text-base text-white">
                      <span className="mr-2 font-mono text-green-400">{hypothesis.id}</span>
                      {hypothesis.name}
                    </h3>
                    <span
                      className={cn(
                        "rounded-full border px-2.5 py-0.5 text-[10px] font-black tracking-[0.12em] uppercase",
                        verdictStyle[hypothesis.verdict],
                      )}
                    >
                      {hypothesis.label}
                    </span>
                  </div>
                  <p className="mt-2 text-sm leading-relaxed text-slate-400">{hypothesis.result}</p>
                </div>
              </Reveal>
            ))}
          </div>
        </Section>

        {/* Stack */}
        <Section
          icon={Layers}
          eyebrow="The FrankenSuite"
          title="Built on the [Franken] stack."
          kicker="FrankenLife composes the suite rather than re-implementing it, and some of its work lands upstream."
        >
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            {stack.map((part, index) => (
              <Reveal key={part.name} delay={(index % 2) * 0.05}>
                <a
                  href={part.href}
                  className="kinetic-card glass-modern flex h-full flex-col rounded-2xl p-6 focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
                >
                  <span className="flex items-center justify-between gap-2 text-lg font-black text-white">
                    {part.name}
                    <ExternalLink className="h-4 w-4 text-slate-500" aria-hidden="true" />
                  </span>
                  <span className="mt-2 text-sm leading-relaxed text-slate-400">{part.role}</span>
                </a>
              </Reveal>
            ))}
          </div>
        </Section>

        {/* Try it */}
        <Section
          id="try"
          icon={Terminal}
          eyebrow="Laboratory mode"
          title="Run it [yourself]."
          kicker="Every command prints versioned NDJSON. Start with what your machine can run, then the 4,096-organism reference world."
        >
          <div className="space-y-3">
            {commands.map((entry) => (
              <Reveal key={entry.command}>
                <CopyCommand comment={entry.comment} command={entry.command} />
              </Reveal>
            ))}
          </div>
        </Section>
      </main>

      <footer className="relative isolate overflow-hidden border-t border-green-500/10">
        <Glow />
        <div className="mx-auto max-w-7xl px-4 py-16 sm:px-6 md:py-24">
          <div className="grid grid-cols-1 gap-10 lg:grid-cols-12">
            <div className="space-y-5 lg:col-span-7">
              <div className="inline-flex items-center gap-2">
                <Microscope className="h-4 w-4 text-green-400" aria-hidden="true" />
                <MicroLabel>Inspiration, not a port</MicroLabel>
              </div>
              <p className="max-w-2xl text-sm leading-relaxed text-slate-400">{inspiration}</p>
              <p className="text-3xl text-white sm:text-4xl">
                <GreenTitle text="The world is [alive]." />
              </p>
            </div>
            <nav aria-label="Project" className="lg:col-span-5">
              <ul className="grid grid-cols-2 gap-3 text-sm">
                <FooterLink href={siteConfig.github} label="Repository" />
                <FooterLink href={siteConfig.plan} label="The design plan" />
                <FooterLink href={siteConfig.fl048} label="Experiment record" />
                <FooterLink href={siteConfig.license} label="License (MIT + rider)" />
              </ul>
            </nav>
          </div>
        </div>
      </footer>
    </>
  );
}

function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex flex-col gap-0.5 sm:flex-row sm:items-baseline sm:justify-between sm:gap-4">
      <dt className="shrink-0 text-xs text-slate-500">{label}</dt>
      <dd className={cn("min-w-0 text-slate-200 sm:text-right", mono && "font-mono text-[13px] break-all")}>
        {value}
      </dd>
    </div>
  );
}

function FooterLink({ href, label }: { href: string; label: string }) {
  return (
    <li>
      <a
        href={href}
        className="flex items-center justify-between gap-2 rounded-xl border border-white/10 px-4 py-3 text-slate-300 transition hover:border-green-400/40 hover:text-green-300 focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
      >
        {label} <ExternalLink className="h-3.5 w-3.5" aria-hidden="true" />
      </a>
    </li>
  );
}
