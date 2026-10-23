import { useEffect, useLayoutEffect, useState } from "react";
import { TitleBar } from "../components/TitleBar";
import { AppContextMenu } from "../features/relay/components/ContextMenu";
import { AppTooltips } from "../features/relay/components/AppTooltips";
import { getPlatform, platformFromUserAgent, type Platform } from "../platform/desktop";
import "../styles.css";
import { RelayApp } from "./RelayApp";

export function App() {
  const [platform, setPlatform] = useState<Platform | "unknown">(() => platformFromUserAgent(navigator.userAgent));
  const [starting, setStarting] = useState(() => document.documentElement.dataset["startupReady"] !== "true");

  useLayoutEffect(() => {
    // Keep native window controls usable while the workspace waits for data.
    document.getElementById("root")!.inert = false;
    const revealWorkspace = () => setStarting(false);
    window.addEventListener("zenith-startup-ready", revealWorkspace);
    return () => window.removeEventListener("zenith-startup-ready", revealWorkspace);
  }, []);

  useLayoutEffect(() => {
    const preventChromeSelectAll = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() !== "a" || (!event.ctrlKey && !event.metaKey)) return;
      const eventTarget = event.target;
      if (eventTarget instanceof HTMLElement && eventTarget.closest("input, textarea, select, [contenteditable=\"true\"]")) return;
      event.preventDefault();
      window.getSelection()?.removeAllRanges();
    };
    document.addEventListener("keydown", preventChromeSelectAll, true);
    return () => document.removeEventListener("keydown", preventChromeSelectAll, true);
  }, []);

  useEffect(() => {
    getPlatform().then(setPlatform).catch(() => undefined);
  }, []);

  return (
    <main className={`app platform-${platform}`}>
      <TitleBar platform={platform} />
      <div className="app-workspace" inert={starting}>
        <RelayApp />
        <AppContextMenu />
        <AppTooltips />
      </div>
    </main>
  );
}
