/** The HTML loader stays usable even when the React bundle cannot start. */
export function setStartupPhase(phase: "runtime" | "error") {
  window.dispatchEvent(new CustomEvent("zenith-startup-phase", { detail: phase }));
}

export function revealStartupShell() {
  window.dispatchEvent(new Event("zenith-startup-ready"));
}
