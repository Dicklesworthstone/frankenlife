"use client";

import { type KeyboardEvent, type ReactNode, useId, useRef, useState } from "react";

import { DemoFrame } from "@/components/explain/demo";
import {
  BirthPanel,
  LogitPanel,
  MixturePanel,
  SignalsPanel,
  ToxicityPanel,
  UptakePanel,
  WrapPanel,
} from "@/components/explain/fixture-panels";
import { cn } from "@/lib/utils";

type Tab = {
  id: string;
  fixture: string;
  label: string;
  title: string;
  Panel: () => ReactNode;
  /** Where the rule is stated, where it is implemented, and the Rust test that checks the same answer. */
  source: ReactNode;
};

const code = (text: string) => <code className="text-slate-400">{text}</code>;

const TABS: Tab[] = [
  {
    id: "c1",
    fixture: "C.1",
    label: "Who eats",
    title: "A crowded cell: two passes, not first come, first served",
    Panel: UptakePanel,
    source: (
      <>
        Plan §5.2 · fixture C.1 · INV-04 · implemented in {code("crates/flife-field/src/uptake.rs")} · checked by{" "}
        {code("crates/flife-field/tests/uptake_ledger.rs::c1_contended_uptake_balances_exactly")}
      </>
    ),
  },
  {
    id: "c3",
    fixture: "C.3",
    label: "Signals vs food",
    title: "Signals cannot counterfeit food",
    Panel: SignalsPanel,
    source: (
      <>
        Plan §4.2 and Appendix D · fixture C.3 · INV-02 · deposits carry energy and moment as one packet in{" "}
        {code("crates/flife-field/src/deposit.rs")} · checked by{" "}
        {code(
          "crates/flife-engine/tests/organism_phase.rs::c3_composition_is_read_from_the_snapshot_and_neutral_cells_are_mildly_harmful",
        )}
      </>
    ),
  },
  {
    id: "c2",
    fixture: "C.2",
    label: "Toxicity",
    title: "Poison costs what the organism has, not what it owes",
    Panel: ToxicityPanel,
    source: (
      <>
        Plan §5.3 · fixture C.2 · INV-01, INV-03 · implemented in {code("crates/flife-engine/src/chemistry.rs")} ·
        checked by {code("crates/flife-engine/tests/organism_phase.rs::c2_toxicity_is_clipped_to_what_the_organism_holds")}
      </>
    ),
  },
  {
    id: "c9",
    fixture: "C.9",
    label: "Packets vs mixture",
    title: "Chemistry runs on each packet, never on the mixture",
    Panel: MixturePanel,
    source: (
      <>
        Plan §5.3, §11.3 · fixture C.9 · implemented in {code("crates/flife-engine/src/chemistry.rs")} · checked by{" "}
        {code("crates/flife-engine/tests/organism_phase.rs::c9_chemistry_is_evaluated_per_packet_not_on_the_mixture")}
      </>
    ),
  },
  {
    id: "c11",
    fixture: "C.11",
    label: "Sigmoid once",
    title: "The last layer is linear; sigmoid is applied exactly once",
    Panel: LogitPanel,
    source: (
      <>
        Plan §6.2–§6.3 · fixture C.11 · decoded in {code("crates/flife-neural/src/actions.rs")} · checked by{" "}
        {code("crates/flife-neural/tests/controller.rs::c11_zero_weights_pass_output_biases_through_a_linear_layer")}
      </>
    ),
  },
  {
    id: "c15",
    fixture: "C.15",
    label: "Wrap-around",
    title: "On a torus, the shortest way round can be the wrong way",
    Panel: WrapPanel,
    source: (
      <>
        Plan §13.5 · fixture C.15 · implemented in {code("crates/flife-render/src/pairing.rs")} · checked by{" "}
        {code(
          "crates/flife-render/tests/interpolation.rs::c15_an_ambiguous_gap_snaps_to_the_latest_state_and_draws_no_reverse_path",
        )}
      </>
    ),
  },
  {
    id: "c5",
    fixture: "C.5",
    label: "Birth",
    title: "A birth commits whole, or charges nothing",
    Panel: BirthPanel,
    source: (
      <>
        Plan §7.4 · fixture C.5 · INV-06 · implemented in {code("crates/flife-engine/src/reproduction.rs")} · checked by{" "}
        {code("crates/flife-engine/tests/emission_births.rs::c5_birth_splits_energy_and_dissipates_the_birth_cost")} and{" "}
        {code("c5_a_rejected_allocation_commits_no_child_no_transfer_and_no_cost")}
      </>
    ),
  },
];

/**
 * "Check it by hand": seven of the plan's seventeen hand-computable fixtures
 * (Appendix C), each a rule beside the plausible wrong version it exists to
 * catch, recomputed in the reader's browser on the fixture's own numbers.
 */
export function FixtureBench() {
  const [selected, setSelected] = useState(0);
  const tabs = useRef<(HTMLButtonElement | null)[]>([]);
  const base = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const tab = TABS[selected] ?? TABS[0];
  if (!tab) return null;
  const { Panel } = tab;

  const choose = (index: number) => {
    const next = (index + TABS.length) % TABS.length;
    setSelected(next);
    tabs.current[next]?.focus();
  };
  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    const keys: Record<string, () => void> = {
      ArrowRight: () => choose(selected + 1),
      ArrowDown: () => choose(selected + 1),
      ArrowLeft: () => choose(selected - 1),
      ArrowUp: () => choose(selected - 1),
      Home: () => choose(0),
      End: () => choose(TABS.length - 1),
    };
    const action = keys[event.key];
    if (action) {
      event.preventDefault();
      action();
    }
  };

  return (
    <DemoFrame
      eyebrow="Appendix C · 7 of the plan's 17 known-answer fixtures"
      title="Every rule has a plausible wrong version. Try both."
      basis="rule"
      source={tab.source}
    >
      <p className="max-w-3xl text-sm leading-relaxed text-slate-400">
        Each tab is one of the plan&apos;s hand-computable fixtures, recomputed here in your browser, in double
        precision, beside the wrong version it exists to catch. Move the numbers and watch the two part ways; reset to
        the fixture and the page checks its answer against the one the plan states. This is the arithmetic, not
        FrankenLife&apos;s code: its Rust kernels are tested against the same answers, and each tab names the test.
      </p>
      <div role="tablist" aria-label="Fixtures" className="mt-5 flex flex-wrap gap-1.5">
        {TABS.map((entry, index) => {
          const on = index === selected;
          return (
            <button
              key={entry.id}
              ref={(element) => {
                tabs.current[index] = element;
              }}
              id={`${base}-tab-${entry.id}`}
              type="button"
              role="tab"
              aria-selected={on}
              aria-controls={`${base}-panel`}
              tabIndex={on ? 0 : -1}
              onClick={() => setSelected(index)}
              onKeyDown={onKeyDown}
              className={cn(
                "flex items-baseline gap-1.5 rounded-xl border px-3 py-2 text-xs font-bold transition focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none",
                on
                  ? "border-green-400/50 bg-green-500/15 text-white"
                  : "border-white/10 bg-black/30 text-slate-300 hover:border-white/25 hover:text-white",
              )}
            >
              <span className={cn("font-mono text-[11px]", on ? "text-green-300" : "text-slate-500")}>{entry.fixture}</span>
              {entry.label}
            </button>
          );
        })}
      </div>
      <div
        id={`${base}-panel`}
        role="tabpanel"
        aria-labelledby={`${base}-tab-${tab.id}`}
        tabIndex={0}
        className="mt-5 rounded-2xl focus-visible:ring-2 focus-visible:ring-green-400 focus-visible:outline-none"
      >
        <h5 className="mb-4 text-base font-black text-white sm:text-lg">
          <span className="mr-2 font-mono text-green-400">{tab.fixture}</span>
          {tab.title}
        </h5>
        <Panel key={tab.id} />
      </div>
    </DemoFrame>
  );
}
