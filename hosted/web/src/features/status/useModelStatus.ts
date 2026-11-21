import { useEffect, useState } from "react";
import { ControlError, observeControl } from "../../api/control";
import { readModelStatus, type ModelStatusSnapshot } from "../../api/modelStatus";
import type { StatusPeriod } from "./history";

type Result = { key: string; data?: ModelStatusSnapshot; failed?: true };

export function useModelStatus(live: boolean, period: StatusPeriod) {
  const [reload, setReload] = useState(0);
  const [result, setResult] = useState<Result>();
  const key = `${period}:${reload}`;
  useEffect(() => {
    if (!live) return;
    const controller = new AbortController();
    const stop = observeControl(() => {
      setResult({ key }); // Never show previously granted rows during reauthorization.
      return readModelStatus(period === "day" ? 24 : 168,
        AbortSignal.any([controller.signal, AbortSignal.timeout(12000)]));
    }, data => setResult({ key, data }), error => {
      setResult({ key, failed: true });
      if (error instanceof ControlError && error.status === 401) location.replace("/login");
    });
    return () => { stop(); controller.abort(); };
  }, [live, period, key]);
  return { result: result?.key === key ? result : undefined, refresh: () => setReload(n => n + 1) };
}
