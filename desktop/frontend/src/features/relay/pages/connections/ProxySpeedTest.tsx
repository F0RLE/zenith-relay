import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Gauge } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { ProxySpeedTestResult } from "../../api/types";
import { IconButton } from "../../components/Ui";

export function ProxySpeedTest({ proxyId, disabled, children }: {
  proxyId: string;
  disabled: boolean;
  children: (action: ReactNode, value: ReactNode) => ReactNode;
}) {
  const { t, i18n } = useTranslation();
  const [pending, setPending] = useState(false);
  const [queued, setQueued] = useState(false);
  const [result, setResult] = useState<ProxySpeedTestResult | null>(null);
  const inFlight = useRef(false);
  const active = useRef(false);
  useEffect(() => {
    active.current = true;
    return () => { active.current = false; };
  }, []);
  const run = async () => {
    if (disabled || inFlight.current) return;
    inFlight.current = true;
    setPending(true);
    setQueued(true);
    setResult(null);
    let next: ProxySpeedTestResult;
    try {
      next = await relayCommands.testStoredProxySpeed(proxyId, () => {
        if (active.current) setQueued(false);
      });
    } catch {
      next = { proxyId, downloadedBytes: 0, elapsedMs: 0, downloadMbps: null, errorCode: "proxy_check_unavailable" };
    }
    inFlight.current = false;
    if (!active.current) return;
    setResult(next);
    setPending(false);
    setQueued(false);
  };
  const speed = result?.downloadMbps;
  const valid = result?.proxyId === proxyId && !result.errorCode && typeof speed === "number" && Number.isFinite(speed) && speed > 0;
  const title = pending ? t(queued ? "proxies.speedQueued" : "proxies.speedTesting") : result
    ? valid ? t("proxies.speedValue", { value: new Intl.NumberFormat(i18n.resolvedLanguage ?? i18n.language, { maximumFractionDigits: 1 }).format(speed) })
      : t(`proxies.speedErrors.${result.errorCode ?? "proxy_check_invalid_response"}`, { defaultValue: t("proxies.speedFailed") })
    : t("proxies.testSpeed");
  const state = pending ? queued ? "queued" : "pending" : result ? valid ? "success" : "error" : "idle";
  return children(<IconButton
    className="proxy-speed-test"
    label={t("proxies.testSpeed")}
    title={title}
    icon={<Gauge className={pending && !queued ? "spin" : undefined} aria-hidden />}
    aria-busy={pending}
    disabled={disabled || pending}
    data-state={state}
    onClick={() => void run()}
  />, <span className="proxy-speed-value" data-state={state} data-relay-tooltip={title} aria-live="polite">{pending || valid ? title : result ? t("proxies.speedFailed") : "—"}</span>);
}
