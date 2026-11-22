import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

const GROUPED = new Intl.NumberFormat("en-US");

/** `12345` as `12,345`, the same on the server and in every browser. */
export function grouped(value: number): string {
  return GROUPED.format(value);
}
