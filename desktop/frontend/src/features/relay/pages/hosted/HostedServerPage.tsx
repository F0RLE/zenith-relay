import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { forgetPairing, getPairing, preparePairing, readAccess, readConnections } from "../../api/hostedDevice";
import type { HostedDeviceReview, HostedReadScope } from "../../api/types/hostedDevice";
import { Button, EmptyState } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { HostedDataPage, HostedPageFrame } from "./HostedDataPage";
import { HostedSettings } from "./HostedSettings";
import { HostedSourceCopy } from "./HostedSourceCopy";
import { hostedErrorKey } from "./hostedErrors";
import "./hosted-server.css";

const scopeByView = { overview: "overview.read", usage: "usage.read" } as const;
type Grant = { expires: number; scopes: HostedReadScope[] };
type DeviceAction = "prepare" | "check" | "forget";

export function HostedServerPage({ children }: { children?: ReactNode }) {
  const { t } = useTranslation();
  const { page: activePage, setPage } = useRelayState();
  const [device, setDevice] = useState<HostedDeviceReview | null>();
  const [grant, setGrant] = useState<Grant>();
  const [error, setError] = useState<unknown>();
  const [action, setAction] = useState<DeviceAction>();
  const [restore, setRestore] = useState(0);
  const running = useRef(false);
  const mounted = useRef(false);
  const page = activePage === "integrations" || activePage === "profiles" ? "overview" : activePage;

  useEffect(() => {
    let current = true;
    mounted.current = true;
    void getPairing().then(saved => { if (current) { setDevice(saved); setError(undefined); } })
      .catch(() => { if (current) setError("device_storage_unavailable"); });
    return () => { current = false; mounted.current = false; };
  }, [restore]);

  const readFailed = useCallback((cause: unknown) => {
    if (cause === "device_target_changed") {
      setGrant(undefined);
      setDevice(undefined);
      setError(undefined);
      setRestore(n => n + 1);
    } else if (cause === "device_not_authorized" || cause === "device_storage_unavailable") {
      setGrant(undefined);
      setError(cause);
    }
  }, []);

  useEffect(() => {
    if (!grant) return;
    const expire = () => { if (grant.expires <= Date.now()) readFailed("device_not_authorized"); };
    const timer = window.setTimeout(expire, Math.min(2_147_483_647, Math.max(0, grant.expires - Date.now())));
    document.addEventListener("visibilitychange", expire);
    return () => { window.clearTimeout(timer); document.removeEventListener("visibilitychange", expire); };
  }, [grant, readFailed]);

  async function run(next: DeviceAction, invitation?: string) {
    if (running.current) return;
    running.current = true;
    setAction(next);
    setError(undefined);
    try {
      if (next === "prepare" && device === null && invitation) {
        setDevice(await preparePairing(invitation));
      } else if (device && next === "forget") {
        await forgetPairing(device.invitation);
        setDevice(null);
        setGrant(undefined);
      } else if (device && next === "check") {
        const access = await readAccess(device.invitation);
        if (!mounted.current) return;
        // Earlier servers have a connections-only grant and no access endpoint.
        const expiry = (access ?? await readConnections(device.invitation)).expires_at_s * 1000;
        if (expiry <= Date.now()) throw "device_not_authorized";
        setGrant({ expires: expiry, scopes: access?.result ?? [] });
      }
    } catch (cause) {
      if (next === "check") setGrant(undefined);
      if (cause === "device_target_changed") readFailed(cause);
      else setError(cause);
    } finally {
      running.current = false;
      setAction(undefined);
    }
  }

  const access = grant && grant.expires > Date.now() ? grant : undefined;
  if (page === "help") return children;
  if (page === "settings") return <HostedSettings device={device} expires={access?.expires} error={error} action={action}
    onPrepare={invitation => void run("prepare", invitation)} onCheck={() => void run("check")} onForget={() => void run("forget")}
    onRetry={() => { setError(undefined); setRestore(n => n + 1); }} onConnections={() => setPage("connections")} />;

  const scope = page === "overview" || page === "usage" ? scopeByView[page] : null;
  const origin = device?.destination.origin;
  const copy = page === "connections" && device && origin?.startsWith("https://") &&
    <HostedSourceCopy key={device.invitation} invitation={device.invitation} />;
  return device && access && (!scope || access.scopes.includes(scope)) ?
    <HostedDataPage key={`${device.invitation}:${page}`} page={page} device={device} expires={access.expires} onReadFailure={readFailed}>{copy}</HostedDataPage>
    : <HostedPageFrame page={page} origin={origin}>
      {device === undefined && !error ? <p role="status">{t("common.loading")}</p> :
        <div role={error ? "alert" : undefined}><EmptyState
          title={t(error ? hostedErrorKey(error) : access ? "hosted.scopeMissing" : "hosted.pairFirst")}
          action={<Button onClick={() => setPage("settings")}>{t("hosted.openSettings")}</Button>}
        /></div>}
      {copy}
    </HostedPageFrame>;
}
