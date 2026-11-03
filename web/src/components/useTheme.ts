import { useEffect, useRef, useState } from "react";
import type { MouseEvent } from "react";
import { flushSync } from "react-dom";
import "./theme-transition.css";

export type Theme = "light" | "dark";
const storageKey = "relay-hosted-theme";

export function useTheme() {
  const [theme, setTheme] = useState<Theme>(() => {
    try {
      const saved = localStorage.getItem(storageKey);
      if (saved === "light" || saved === "dark") return saved;
    } catch { /* Theme storage is optional. */ }
    return matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  });
  const current = useRef(theme);
  const generation = useRef(0);
  const transition = useRef<ViewTransition | null>(null);
  const animation = useRef<Animation | null>(null);
  const fadeTimer = useRef<number | undefined>(undefined);

  useEffect(() => () => {
    generation.current++;
    transition.current?.skipTransition();
    animation.current?.cancel();
    window.clearTimeout(fadeTimer.current);
    document.documentElement.removeAttribute("data-theme-transition");
    document.documentElement.removeAttribute("data-theme-fade");
  }, []);

  function toggleTheme(event?: MouseEvent<HTMLButtonElement>) {
    const next = current.current === "light" ? "dark" : "light";
    current.current = next;
    const token = ++generation.current;
    transition.current?.skipTransition();
    animation.current?.cancel();
    window.clearTimeout(fadeTimer.current);
    const root = document.documentElement;
    root.removeAttribute("data-theme-transition");
    root.removeAttribute("data-theme-fade");
    let applied = false;
    const apply = () => {
      if (generation.current !== token || applied) return;
      applied = true;
      flushSync(() => setTheme(next));
      try { localStorage.setItem(storageKey, next); } catch { /* Keep the current page usable. */ }
    };
    const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;
    const fallback = () => {
      if (generation.current !== token) return;
      root.removeAttribute("data-theme-transition");
      if (!reducedMotion) {
        root.setAttribute("data-theme-fade", "");
        fadeTimer.current = window.setTimeout(() => root.removeAttribute("data-theme-fade"), 200);
      }
      apply();
    };
    if (reducedMotion || !document.startViewTransition) { fallback(); return; }

    const box = event?.currentTarget.getBoundingClientRect();
    const x = Math.max(0, Math.min(innerWidth, box ? box.left + box.width / 2 : innerWidth / 2));
    const y = Math.max(0, Math.min(innerHeight, box ? box.top + box.height / 2 : innerHeight / 2));
    const radius = Math.hypot(Math.max(x, innerWidth - x), Math.max(y, innerHeight - y)) + 1;
    // Capture complete palettes, not a mixture of per-button color transitions.
    root.setAttribute("data-theme-transition", "circle");
    try {
      const active = document.startViewTransition(apply);
      transition.current = active;
      void active.ready.then(() => {
        if (generation.current !== token) return;
        animation.current = root.animate({ clipPath: [`circle(0px at ${x}px ${y}px)`, `circle(${radius}px at ${x}px ${y}px)`] }, {
          duration: 450, easing: "cubic-bezier(.22, 1, .36, 1)", pseudoElement: "::view-transition-new(root)",
        });
        return animation.current.finished;
      }).catch(() => { /* A hidden tab or a newer toggle may skip the reveal. */ });
      void active.finished.catch(fallback).finally(() => {
        if (generation.current !== token) return;
        root.removeAttribute("data-theme-transition");
        transition.current = null;
        animation.current = null;
      });
    } catch { fallback(); }
  }
  return { theme, toggleTheme };
}
