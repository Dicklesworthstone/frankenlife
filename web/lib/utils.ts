import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** `12345` as `12,345`, the same on the server and in every browser. */
export function grouped(value: number): string {
  return value.toLocaleString("en-US");
}
