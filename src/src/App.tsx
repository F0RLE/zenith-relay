import { useEffect, useState } from "react";
import { TitleBar } from "./components/TitleBar";
import { RelayApp } from "./features/relay/app/RelayApp";
import { getPlatform, Platform } from "./tauri";
import "./styles.css";

export function App() {
  const [platform, setPlatform] = useState<Platform>("windows");
  useEffect(() => { getPlatform().then(setPlatform).catch(() => setPlatform("windows")); }, []);
  return <main className={`app platform-${platform}`}><TitleBar platform={platform} /><RelayApp /></main>;
}
