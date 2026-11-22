// A basis badge says the whole of its label, and may break only after a " · ":
// each part between the dots is kept on one line.

import { describe, expect, test } from "bun:test";
import { renderToString } from "react-dom/server";

import { BASIS, type Basis, BasisBadge } from "./demo";

/** The text a reader sees: tags and React's text separators removed. */
function text(html: string): string {
  return html.replace(/<!-- -->/g, "").replace(/<[^>]+>/g, "");
}

describe("basis badges", () => {
  for (const basis of Object.keys(BASIS) as Basis[]) {
    test(`${basis}: the full label, unbreakable between its dots`, () => {
      const label = BASIS[basis].label;
      const html = renderToString(<BasisBadge basis={basis} />);
      expect(text(html)).toBe(label);
      const parts = label.split(" · ");
      const kept = [...html.matchAll(/<span class="whitespace-nowrap">(.*?)<\/span>/g)].map((match) => text(match[1] ?? ""));
      expect(kept).toEqual(parts.map((part, index) => (index < parts.length - 1 ? `${part} ·` : part)));
    });
  }
});
