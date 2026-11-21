import { Moon, Sun } from "./Icons";
import type { Theme } from "./useTheme";

/** Keep one correctly sized glyph; the page transition carries the animation. */
export function ThemeIcon({ theme }: { theme: Theme }) {
  const Icon = theme === "light" ? Sun : Moon;
  return <Icon className="theme-icon" data-current-theme={theme} aria-hidden="true" />;
}
