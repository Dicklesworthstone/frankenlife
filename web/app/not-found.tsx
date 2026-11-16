import { FrankenContainer, GreenTitle, MicroLabel } from "@/components/franken";
import { siteConfig } from "@/lib/content";

export const metadata = { title: `Not found · ${siteConfig.name}` };

export default function NotFound() {
  return (
    <main className="flex min-h-screen items-center justify-center px-4">
      <FrankenContainer className="max-w-lg p-10 text-center">
        <MicroLabel>404 · no such tick</MicroLabel>
        <h1 className="mt-5 text-4xl text-white">
          <GreenTitle text="Nothing [lives] here." />
        </h1>
        <p className="mt-4 text-sm leading-relaxed text-slate-400">
          This page was never committed. The world is still where you left it.
        </p>
        <a
          href="/"
          className="mt-8 inline-flex items-center rounded-2xl bg-green-500 px-5 py-3 text-sm font-black text-black transition hover:bg-green-400 focus-visible:ring-2 focus-visible:ring-green-300 focus-visible:outline-none"
        >
          Back to the film
        </a>
      </FrankenContainer>
    </main>
  );
}
