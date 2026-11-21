import { useEffect, useState } from "react";
import { ControlError } from "../../api/control";
import { readPoolActivity, type PoolActivity } from "../../api/poolActivity";

export function usePoolActivity(refresh: number) {
  const [view, setView] = useState<PoolActivity>();
  useEffect(() => {
    let pending: AbortController | undefined;
    let timer: number | undefined;
    const stop = () => {
      window.clearTimeout(timer);
      const request = pending;
      pending = undefined;
      request?.abort();
    };
    const refresh = async () => {
      if (document.hidden || pending) return;
      const request = new AbortController();
      pending = request;
      let watch = true;
      try {
        const next = await readPoolActivity(AbortSignal.any([request.signal, AbortSignal.timeout(4000)]));
        if (request === pending) setView(next);
      } catch (error) {
        if (request !== pending) return;
        setView(undefined);
        if (error instanceof ControlError && error.status === 401) {
          watch = false;
          location.replace("/admin/login");
        }
      } finally {
        if (request === pending) {
          pending = undefined;
          if (watch && !document.hidden) timer = window.setTimeout(refresh, 2000);
        }
      }
    };
    const visibility = () => {
      stop();
      if (document.hidden) setView(undefined);
      else void refresh();
    };
    document.addEventListener("visibilitychange", visibility);
    void refresh();
    return () => {
      document.removeEventListener("visibilitychange", visibility);
      stop();
    };
  }, [refresh]);
  return view;
}
