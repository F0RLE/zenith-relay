import { useEffect, useLayoutEffect, useState } from "react";
import { TitleBar } from "../components/TitleBar";
import { AppContextMenu } from "../features/relay/components/ContextMenu";
import { getPlatform, type Platform } from "../platform/desktop";
import "../styles.css";
import { RelayApp } from "./RelayApp";

export function App() {
  const [platform, setPlatform] = useState<Platform>("windows");

  useLayoutEffect(() => {
    const preventChromeSelectAll = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() !== "a" || (!event.ctrlKey && !event.metaKey)) return;
      const target = event.target;
      if (target instanceof HTMLElement && target.closest("input, textarea, select, [contenteditable=\"true\"]")) return;
      event.preventDefault();
      window.getSelection()?.removeAllRanges();
    };
    document.addEventListener("keydown", preventChromeSelectAll, true);
    return () => document.removeEventListener("keydown", preventChromeSelectAll, true);
  }, []);

  useEffect(() => {
    getPlatform().then(setPlatform).catch(() => setPlatform("windows"));
  }, []);

  return (
    <main className={`app platform-${platform}`}>
      <TitleBar platform={platform} />
      <RelayApp />
      <AppContextMenu />
    </main>
  );
}
