import { revealWindowAfterBackgroundColor } from "./platform/desktop";
import { setStartupPhase } from "./platform/startup";

// Show the HTML loader without waiting for React or the workspace chunks.
const background = getComputedStyle(document.documentElement).getPropertyValue("--bootstrap-bg").trim();
void revealWindowAfterBackgroundColor(background);
void import("./main").catch(() => setStartupPhase("error"));
