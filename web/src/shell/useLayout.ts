import { useSyncExternalStore } from "react";

export type Layout = "desktop" | "tablet" | "phone";

const desktop = "(min-width: 1024px)";
const tablet = "(min-width: 640px)";

function current(): Layout {
  if (window.matchMedia(desktop).matches) return "desktop";
  return window.matchMedia(tablet).matches ? "tablet" : "phone";
}

function subscribe(onChange: () => void) {
  const queries = [desktop, tablet].map((q) => window.matchMedia(q));
  for (const q of queries) q.addEventListener("change", onChange);
  return () => {
    for (const q of queries) q.removeEventListener("change", onChange);
  };
}

export const useLayout = (): Layout => useSyncExternalStore(subscribe, current);
