import type { NextConfig } from "next";

// A static export: `next build` writes `out/`, which Cloudflare serves as a
// Worker's static assets (wrangler.jsonc). The page renders nothing on a
// server; worker.ts only redirects www., types the share images and streams
// the films from R2.
const nextConfig: NextConfig = {
  output: "export",
  images: { unoptimized: true },
  poweredByHeader: false,
  reactStrictMode: true,
  // `next dev` otherwise writes web/AGENTS.md when it sees a coding agent;
  // the repository's AGENTS.md is the agents' contract.
  agentRules: false,
};

export default nextConfig;
