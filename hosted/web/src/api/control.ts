export const uuidText = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
export const csrfText = /^[0-9a-f]{64}$/;
export const record = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

export class ControlError extends Error {
  constructor(readonly status: number, readonly code?: string, readonly details?: Record<string, unknown>) {
    super("Control request failed");
  }
}

/** Re-read on returning to the tab, without a heartbeat that prolongs idle sessions. */
export function observeControl<T>(read: () => Promise<T>, updated: (v: T) => void, failed: (e: unknown) => void) {
  let mounted = true, running = false;
  const refresh = async () => {
    if (running) return;
    running = true;
    try { const v = await read(); if (mounted) updated(v); }
    catch (e) { if (mounted) failed(e); }
    finally { running = false; }
  };
  const returned = () => { if (!document.hidden) void refresh(); };
  void refresh();
  window.addEventListener("focus", returned);
  document.addEventListener("visibilitychange", returned);
  return () => {
    mounted = false;
    window.removeEventListener("focus", returned);
    document.removeEventListener("visibilitychange", returned);
  };
}

export async function controlResponse(path: string, init: RequestInit = {}) {
  if (path.length > 8192
    || !/^[a-z][a-z0-9_-]*(?:\/[a-z0-9_-]+)*(?:\?[a-zA-Z0-9_~.*%=&+:-]*)?$/.test(path)) throw new Error("Invalid control path");
  const r = await fetch("/api/hosted/v1/" + path, {
    ...init,
    mode: "same-origin", credentials: init.credentials === "omit" ? "omit" : "same-origin", redirect: "error", cache: "no-store",
    signal: init.signal ? AbortSignal.any([init.signal, AbortSignal.timeout(12000)]) : AbortSignal.timeout(12000),
  });
  if (!r.ok) {
    const body: unknown = await r.json().catch(() => null);
    const error = record(body) && record(body.error) ? body.error : undefined;
    const code = error?.code;
    throw new ControlError(r.status, typeof code === "string" && /^[a-z_]{1,48}$/.test(code) ? code : undefined,
      record(error?.details) ? error.details : undefined);
  }
  return r;
}

export async function control(path: string, init: RequestInit = {}) {
  const r = await controlResponse(path, init);
  if (r.headers.get("content-type")?.split(";")[0] !== "application/json") throw new Error("Invalid control response");
  const v: unknown = await r.json();
  if (!record(v) || !record(v.meta) || v.meta.api_version !== "1" || typeof v.meta.request_id !== "string"
    || !uuidText.test(v.meta.request_id) || !record(v.data)) throw new Error("Invalid control envelope");
  return v.data;
}
