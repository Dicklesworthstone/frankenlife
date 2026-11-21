// The fixture panels, the life cycle and the controller render on the server
// without throwing, the same bytes every time, and, before a browser measures
// them, at the width each figure was drawn for: a reader without JavaScript
// sees the figures in their original proportions.

import { describe, expect, test } from "bun:test";
import type { ReactElement } from "react";
import { renderToString } from "react-dom/server";

import { BrainAnatomy } from "./brain";
import { BirthPanel, LogitPanel, MixturePanel, SignalsPanel, ToxicityPanel, UptakePanel, WrapPanel } from "./fixture-panels";
import { LifeCycle } from "./life-cycle";

/** Every viewBox in `html`, in order. */
function viewBoxes(html: string): string[] {
  return [...html.matchAll(/viewBox="([^"]+)"/g)].map((match) => match[1] ?? "");
}

function rendered(element: ReactElement): string {
  const html = renderToString(element);
  expect(renderToString(element)).toBe(html);
  return html;
}

describe("figures before they are measured", () => {
  test("C.1: both cells at 300 across", () => {
    const boxes = viewBoxes(rendered(<UptakePanel />)).filter((box) => box.startsWith("0 0 300 "));
    expect(boxes).toHaveLength(2);
  });

  test("C.3: 480 across, the fixture's cell of energy 8", () => {
    const html = rendered(<SignalsPanel />);
    expect(viewBoxes(html)).toContain("0 0 480 214");
    expect(html).toContain("The cell");
  });

  test("C.2: 470 across, the rule's organism exhausted and the wrong one at −3", () => {
    const html = rendered(<ToxicityPanel />);
    expect(viewBoxes(html)).toContain("0 0 470 128");
    expect(html).toContain("0: exhausted, it dies");
    expect(html).toContain("negative energy: impossible");
  });

  test("C.9 and C.11: 540 across, with the axis titles in full", () => {
    const mixture = rendered(<MixturePanel />);
    expect(viewBoxes(mixture)).toContain("0 0 540 252");
    expect(mixture).toContain("the packet’s chemistry along the organism’s affinity");
    const logit = rendered(<LogitPanel />);
    expect(viewBoxes(logit)).toContain("0 0 540 238");
    expect(logit).toContain("all the wrong version can ever say");
  });

  test("C.15 and C.5 render", () => {
    expect(rendered(<WrapPanel />)).toContain("C.15");
    const birth = viewBoxes(rendered(<BirthPanel />)).filter((box) => box === "0 0 290 76");
    expect(birth).toHaveLength(2);
  });

  test("the life cycle and the controller keep their wide drawings", () => {
    expect(viewBoxes(rendered(<LifeCycle />))).toContain("-74 -10 588 462");
    expect(viewBoxes(rendered(<BrainAnatomy />))).toContain("0 0 640 356");
  });
});
