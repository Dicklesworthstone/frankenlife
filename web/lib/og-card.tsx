// The site's share card (app/opengraph-image.tsx, app/twitter-image.tsx),
// drawn at build time by next/og (Satori, not a browser: inline styles, flex
// on every element, PNG or JPEG images only).
//
// Its picture is the film's social card, a crop of one film frame that
// `flife render` drew (scripts/media.ts records which); the card only darkens
// it beside the words. Every figure it shows is lib/media.json's.

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { ImageResponse } from "next/og";

import { film } from "@/lib/media";

/** The lineage palette (flife-manim's mapping.rs, LINEAGE_PALETTE): the colours the film's organisms wear. */
const LINEAGE = ["#2EC4B6", "#9B5DE5", "#F15BB5", "#9BF6FF", "#FFB627", "#B5E61D", "#FF6F59", "#4EA8DE"];
const INK = "#020a02";

/** A Google font as TrueType, which Satori reads (it cannot read WOFF2). */
async function googleFont(family: string, weight: number): Promise<ArrayBuffer> {
  const query = `${family.replace(/ /g, "+")}:wght@${weight}`;
  const css = await (await fetch(`https://fonts.googleapis.com/css2?family=${query}`)).text();
  const url = /src: url\((\S+?)\) format\('(?:truetype|opentype)'\)/.exec(css)?.[1];
  if (!url) throw new Error(`og-card: no TrueType source for ${family} ${weight}`);
  const font = await fetch(url);
  if (!font.ok) throw new Error(`og-card: ${url} answered ${font.status}`);
  return font.arrayBuffer();
}

/** The social card's JPEG as a data URL, read from public/ (scripts/check-media.ts has checked its bytes). */
function picture(): string {
  const bytes = readFileSync(join(process.cwd(), "public", film.social.src));
  return `data:image/jpeg;base64,${bytes.toString("base64")}`;
}

const thousands = (n: number) => n.toLocaleString("en-US");

/** The share card at `width × height` (1200 × 630 for Open Graph, 1200 × 600 for X). */
export async function shareCard(width: number, height: number): Promise<ImageResponse> {
  const [display, body, mono] = await Promise.all([
    googleFont("Inter", 800),
    googleFont("Inter", 500),
    googleFont("JetBrains Mono", 500),
  ]);
  const { world, social } = film;
  const tick = world.ticks[social.frame] ?? world.from.tick;
  const live = world.live[social.frame] ?? world.live[0] ?? 0;
  // The card's crop is 1200 × 630; at 600 tall it is centred.
  const offset = Math.round((height - social.height) / 2);

  return new ImageResponse(
    <div
      style={{
        width,
        height,
        display: "flex",
        position: "relative",
        background: INK,
        fontFamily: "Inter",
        overflow: "hidden",
      }}
    >
      {/* The film frame, full bleed. */}
      <img
        src={picture()}
        width={social.width}
        height={social.height}
        alt=""
        style={{ position: "absolute", left: 0, top: offset, width: social.width, height: social.height }}
      />
      {/* Dark beside the words, the tissue clear on the right. */}
      <div
        style={{
          position: "absolute",
          left: 0,
          top: 0,
          width,
          height,
          display: "flex",
          backgroundImage:
            "linear-gradient(90deg, rgba(2,10,2,0.97) 0%, rgba(2,10,2,0.94) 36%, rgba(2,10,2,0.7) 50%, rgba(2,10,2,0.18) 66%, rgba(2,10,2,0) 78%)",
        }}
      />
      <div
        style={{
          position: "absolute",
          left: 0,
          top: 0,
          width,
          height,
          display: "flex",
          backgroundImage:
            "linear-gradient(180deg, rgba(2,10,2,0.5) 0%, rgba(2,10,2,0) 20%, rgba(2,10,2,0) 72%, rgba(2,10,2,0.82) 100%)",
        }}
      />
      {/* A green glow behind the title. */}
      <div
        style={{
          position: "absolute",
          left: -260,
          top: -220,
          width: 820,
          height: 820,
          display: "flex",
          borderRadius: 410,
          backgroundImage: "radial-gradient(circle, rgba(34,197,94,0.20) 0%, rgba(34,197,94,0.06) 42%, rgba(34,197,94,0) 66%)",
        }}
      />

      {/* The words. */}
      <div
        style={{
          position: "absolute",
          left: 64,
          top: 54,
          bottom: 54,
          width: 640,
          display: "flex",
          flexDirection: "column",
          justifyContent: "space-between",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 16 }}>
          {/* The site's icon (app/icon.svg): a cell and two daughters. */}
          <div
            style={{
              width: 52,
              height: 52,
              display: "flex",
              position: "relative",
              borderRadius: 13,
              background: INK,
              border: "1px solid rgba(74,222,128,0.35)",
            }}
          >
            <div
              style={{
                position: "absolute",
                left: 10,
                top: 10,
                width: 32,
                height: 32,
                display: "flex",
                borderRadius: 16,
                background: "radial-gradient(circle at 38% 36%, #bef264 0%, #22c55e 45%, #052e16 100%)",
              }}
            />
            <div
              style={{
                position: "absolute",
                left: 35,
                top: 10,
                width: 8,
                height: 8,
                display: "flex",
                borderRadius: 4,
                background: "#4ade80",
              }}
            />
            <div
              style={{
                position: "absolute",
                left: 10,
                top: 37,
                width: 6,
                height: 6,
                display: "flex",
                borderRadius: 3,
                background: "#22d3ee",
              }}
            />
          </div>
          <div style={{ display: "flex", fontSize: 32, fontWeight: 800, color: "#ffffff", letterSpacing: -0.5 }}>
            FrankenLife
          </div>
        </div>

        <div style={{ display: "flex", flexDirection: "column" }}>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 12,
              fontFamily: "JetBrains Mono",
              fontSize: 17,
              letterSpacing: 3,
              color: "#4ade80",
              marginBottom: 22,
            }}
          >
            <div style={{ display: "flex", width: 10, height: 10, borderRadius: 5, background: "#4ade80" }} />
            ARTIFICIAL LIFE · RENDERED, NOT DRAWN
          </div>
          <div style={{ display: "flex", fontSize: 88, fontWeight: 800, color: "#ffffff", lineHeight: 1, letterSpacing: -3 }}>
            Evolution,
          </div>
          <div
            style={{
              display: "flex",
              fontSize: 88,
              fontWeight: 800,
              lineHeight: 1.08,
              letterSpacing: -3,
              backgroundImage: "linear-gradient(90deg, #bef264 0%, #4ade80 45%, #2dd4bf 100%)",
              backgroundClip: "text",
              color: "transparent",
            }}
          >
            bit for bit.
          </div>
          <div
            style={{
              display: "flex",
              marginTop: 24,
              fontSize: 25,
              fontWeight: 500,
              lineHeight: 1.4,
              color: "#cbd5e1",
              maxWidth: 600,
            }}
          >
            {`${thousands(live)} organisms, each with its own neural network, grow tissue in one shared chemical field.`}
          </div>
        </div>

        <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
          {[`${thousands(live)} organisms`, "own brain each", "deterministic"].map((label) => (
            <div
              key={label}
              style={{
                display: "flex",
                padding: "9px 16px",
                borderRadius: 22,
                fontFamily: "JetBrains Mono",
                fontSize: 17,
                color: "#d9f99d",
                background: "rgba(34,197,94,0.12)",
                border: "1px solid rgba(74,222,128,0.35)",
              }}
            >
              {label}
            </div>
          ))}
        </div>
      </div>

      {/* What the picture is, from the manifest. */}
      <div
        style={{
          position: "absolute",
          right: 28,
          bottom: 30,
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "8px 14px",
          borderRadius: 10,
          fontFamily: "JetBrains Mono",
          fontSize: 15,
          color: "#e2e8f0",
          background: "rgba(2,10,2,0.72)",
          border: "1px solid rgba(255,255,255,0.14)",
        }}
      >
        {`${world.name} · tick ${thousands(tick)} · colour = founder lineage`}
      </div>
      <div
        style={{
          position: "absolute",
          right: 30,
          top: 34,
          display: "flex",
          fontFamily: "JetBrains Mono",
          fontSize: 20,
          color: "#ffffff",
          padding: "6px 12px",
          borderRadius: 8,
          background: "rgba(2,10,2,0.55)",
        }}
      >
        frankenlife.org
      </div>

      {/* The lineage palette, along the bottom edge. */}
      <div
        style={{
          position: "absolute",
          left: 0,
          right: 0,
          bottom: 0,
          height: 6,
          display: "flex",
          backgroundImage: `linear-gradient(90deg, ${LINEAGE.join(", ")})`,
        }}
      />
    </div>,
    {
      width,
      height,
      fonts: [
        { name: "Inter", data: display, weight: 800, style: "normal" },
        { name: "Inter", data: body, weight: 500, style: "normal" },
        { name: "JetBrains Mono", data: mono, weight: 500, style: "normal" },
      ],
    },
  );
}

/** The card's alt text. */
export const shareAlt = `FrankenLife: ${thousands(film.world.live[film.social.frame] ?? film.world.live[0] ?? 0)} simulated organisms, each with its own neural network, grown into tissue and drawn by flife, under the headline "Evolution, bit for bit."`;
