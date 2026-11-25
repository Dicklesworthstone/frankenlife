// One film's record, as its manifest (lib/films/<id>.json) states it: the
// world, the film, and the commands that make it again.

import { Terminal } from "lucide-react";

import { CameraTrack } from "@/components/camera-track";
import { MicroLabel, Reticle } from "@/components/franken";
import { filmNotes } from "@/lib/content";
import type { FilmManifest } from "@/lib/media";
import { grouped } from "@/lib/utils";

function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex flex-col gap-0.5 py-2 sm:flex-row sm:items-baseline sm:justify-between sm:gap-6">
      <dt className="shrink-0 text-xs text-slate-500">{label}</dt>
      <dd className={mono ? "min-w-0 font-mono text-[12.5px] break-all text-slate-200 sm:text-right" : "min-w-0 text-slate-200 sm:text-right"}>
        {value}
      </dd>
    </div>
  );
}

const mib = (bytes: number) => (bytes / (1024 * 1024)).toFixed(1);

/** The zooms the shots reached, in order (keys hold each zoom twice; the whole torus, 1, is not a shot). */
function shotZooms(keys: { zoom: number }[]): string {
  const zooms: number[] = [];
  for (const { zoom } of keys) if (zoom > 1 && zooms.at(-1) !== zoom) zooms.push(zoom);
  return zooms.map((zoom) => `${zoom}×`).join(", ");
}

export function FilmProvenance({ film }: { film: FilmManifest }) {
  const { world, render, video } = film;
  const note = filmNotes[film.id];
  const seconds = (ticks: number) => (ticks * world.timestep[0]) / world.timestep[1];
  const lowest = Math.min(...world.live);
  const highest = Math.max(...world.live);
  const crfOf = (encode: string[]) => {
    const at = encode.indexOf("-crf");
    return at >= 0 ? encode[at + 1] : undefined;
  };
  const crf = crfOf(video.encode);
  const reproduce = [
    "# flife is `cargo run --release -p flife-cli --` in the repository",
    `flife run ${world.preset ?? note?.config ?? "CONFIG"} --ticks ${world.from.tick} --run-dir RUN`,
    render.command.replace("FRAMES", "frames"),
    ...(render.camera ? [`# CAMERA: lib/films/${film.id}.json's render.camera keys, as {"keys": [...]}`] : []),
  ];

  return (
    <article className="panel relative rounded-[1.75rem] p-6 sm:p-8">
      <Reticle inset={9} size={12} />
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h3 className="text-xl font-bold text-white">{note?.title ?? world.name}</h3>
        <span className="font-mono text-[11px] text-green-300/80">{note?.chip ?? world.model}</span>
      </div>
      <div className="mt-4 grid grid-cols-1 gap-x-10 lg:grid-cols-2">
        <div>
          <MicroLabel>The world</MicroLabel>
          <dl className="ruled mt-2 text-sm">
            {world.preset || !note?.config ? (
              <Row label="Preset" value={world.preset ?? world.name} mono />
            ) : (
              <Row label="Configuration" value={note.config} mono />
            )}
            {world.model ? <Row label="Model" value={world.model} mono /> : null}
            {note?.mechanism ? <Row label="v2 mechanism on" value={note.mechanism} mono /> : null}
            <Row label="Seed" value={String(world.seed)} mono />
            <Row label="Founders" value={`${grouped(world.founders)} on a ${world.grid[0]} × ${world.grid[1]} field`} />
            <Row label="Evolved first" value={`${grouped(world.from.tick)} ticks (${seconds(world.from.tick).toFixed(0)} simulated seconds)`} />
            <Row label="Alive in the film" value={lowest === highest ? grouped(lowest) : `${grouped(lowest)} – ${grouped(highest)}`} />
            <Row label="First digest" value={world.from.digest} mono />
            <Row label="Last digest" value={world.to.digest} mono />
          </dl>
        </div>
        <div>
          <MicroLabel>The film</MicroLabel>
          <dl className="ruled mt-2 text-sm">
            <Row label="Ticks" value={`${grouped(world.from.tick)} → ${grouped(world.to.tick)}, a frame every ${world.every}`} />
            <Row label="Frames" value={`${video.frames} at ${video.fps} fps, ${video.width} × ${video.height}`} />
            <Row
              label="File"
              value={
                `one MP4, ${mib(video.bytes)} MiB, H.264 CRF ${crf}` +
                (video.small
                  ? `; phones play ${video.small.width} × ${video.small.height}, ${mib(video.small.bytes)} MiB, CRF ${crfOf(video.small.encode)}`
                  : "")
              }
            />
            <Row label="Colour" value={render.meaning.split(":")[0]?.trim() || render.mapping} />
            <Row label="Field" value={`${render.field_mapping === "none" ? "not drawn" : render.field_mapping}, ${render.look} look`} />
            <Row label="Camera" value={
                !render.camera
                  ? "the whole torus, still"
                  : render.camera.keys.length === 1
                    ? `held on the torus centre at zoom ${render.camera.keys[0]?.zoom}`
                    : `${render.camera.keys.length} keys; centres from the run, zooms ${shotZooms(render.camera.keys)}`
              } />
            <Row label="Revision" value={render.revision.slice(0, 12)} mono />
          </dl>
        </div>
      </div>
      <CameraTrack film={film} />
      <details className="group mt-5 rounded-xl border border-white/[0.06] bg-black/30">
        <summary className="flex min-h-11 cursor-pointer list-none items-center gap-2 px-4 text-sm font-semibold text-slate-300 hover:text-white">
          <Terminal className="h-4 w-4 text-green-400" aria-hidden="true" /> Make it yourself
          <span aria-hidden="true" className="ml-auto font-mono text-xs text-slate-500 group-open:hidden">
            +
          </span>
          <span aria-hidden="true" className="ml-auto hidden font-mono text-xs text-slate-500 group-open:inline">
            −
          </span>
        </summary>
        <pre className="overflow-x-auto px-4 pb-4 text-[12.5px] leading-relaxed text-green-200">
          <code>{reproduce.join("\n")}</code>
        </pre>
        <p className="px-4 pb-4 text-xs leading-relaxed text-slate-400">
          Controllers ran on {render.controllers}; no result depends on the route. The PNG frames are flife&apos;s; the
          page&apos;s video encodes them with flife&apos;s own BT.709 colour chain, named by its SHA-256:{" "}
          <code className="break-all text-slate-300">{video.sha256}</code>.
        </p>
      </details>
    </article>
  );
}
