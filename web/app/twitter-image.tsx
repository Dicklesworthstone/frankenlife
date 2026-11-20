// The X (Twitter) share image, made at build time (lib/og-card.tsx).
import { shareAlt, shareCard } from "@/lib/og-card";

export const dynamic = "force-static";
export const alt = shareAlt;
export const size = { width: 1200, height: 600 };
export const contentType = "image/png";

export default function Image() {
  return shareCard(size.width, size.height);
}
