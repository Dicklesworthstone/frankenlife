// The controller, weights and scale explainers render on the server without
// throwing, the same bytes every time (so hydration never shifts), and show
// the figures they are built to show.

import { describe, expect, test } from "bun:test";
import { renderToString } from "react-dom/server";

import { BrainAnatomy } from "./brain";
import { ScaleCalculator } from "./scale";
import { IndependentWeights } from "./weights";

describe("server rendering", () => {
  test("the controller: the default network's 9,988 parameters, spelled out", () => {
    const html = renderToString(<BrainAnatomy />);
    expect(renderToString(<BrainAnatomy />)).toBe(html);
    expect(html).toContain("9,988");
    expect(html).toContain("104×64 + 64 + 64×32 + 32 + 32×36 + 36");
    expect(html).toContain("No motor output");
  });

  test("the weights: 48 organisms and the heredity record", () => {
    const html = renderToString(<IndependentWeights />);
    expect(renderToString(<IndependentWeights />)).toBe(html);
    expect(html).toContain("99.8%");
    expect(html).toContain("1.3%");
  });

  test("the scale: README's 30-sweep F16 row at the target", () => {
    const html = renderToString(<ScaleCalculator />);
    expect(renderToString(<ScaleCalculator />)).toBe(html);
    expect(html).toContain("3.995 billion");
    expect(html).toContain("240 GB/s");
    expect(html).toContain("7.44 GiB");
    expect(html).toContain("74,793");
    expect(html).toContain("426–748 GB/s");
  });
});
