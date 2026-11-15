import { useEffect, useRef, useState } from "react";
import type { OwnerCatalogSettings } from "../features/owner/modelRules";

type VersionedSettings = { kind: "state"; revision: number; sender: string; settings: OwnerCatalogSettings };

/** Dev-only synthetic state. No durable storage, server write, credentials or session inference. */
export function usePreviewCatalog(initial: OwnerCatalogSettings) {
  const [settings, setSettings] = useState(initial);
  const identity = useRef(crypto.randomUUID());
  const current = useRef<VersionedSettings>({ kind: "state", revision: 0, sender: identity.current, settings: initial });
  const channel = useRef<BroadcastChannel | null>(null);
  useEffect(() => {
    if (typeof BroadcastChannel === "undefined") return;
    const transport = new BroadcastChannel("relay-hosted-preview-catalog-v1");
    channel.current = transport;
    transport.onmessage = ({ data }: MessageEvent<VersionedSettings | { kind: "request" }>) => {
      if (data?.kind === "request") {
        if (current.current.revision > 0) transport.postMessage(current.current);
        return;
      }
      if (data?.kind !== "state" || !Number.isSafeInteger(data.revision) || typeof data.sender !== "string"
        || !Array.isArray(data.settings?.modelDisplayOrder) || !data.settings.modelDisplayOrder.every(id => typeof id === "string")
        || !data.settings.rules || typeof data.settings.rules !== "object") return;
      const previous = current.current;
      if (data.revision < previous.revision || data.revision === previous.revision && data.sender <= previous.sender) return;
      current.current = data;
      setSettings(data.settings);
    };
    transport.postMessage({ kind: "request" });
    return () => { transport.close(); channel.current = null; };
  }, []);
  function onChange(next: OwnerCatalogSettings) {
    const state: VersionedSettings = { kind: "state", revision: current.current.revision + 1, sender: identity.current, settings: next };
    current.current = state;
    setSettings(next);
    channel.current?.postMessage(state);
  }
  return { settings, onChange };
}
