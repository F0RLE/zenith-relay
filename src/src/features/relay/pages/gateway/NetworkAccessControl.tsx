import { useEffect, useId, useState } from "react";
import { Copy, KeyRound, Link2, Network, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands, type LocalNetworkAccess } from "../../api/commands";
import { ActionMenu, ActionMenuItem, CopyButton, IconButton, ToggleSwitch, copyText, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

export function NetworkAccessControl({ port, running }: { port: string; running: boolean }) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const confirm = useConfirm();
  const controlId = useId();
  const [access, setAccess] = useState<LocalNetworkAccess | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => setAccess(null), [port]);
  useEffect(() => {
    if (busy) return;
    const controller = new AbortController();
    const { signal } = controller;
    let pending = false;
    const refresh = async () => {
      if (signal.aborted || pending || document.hidden) return;
      pending = true;
      try {
        const next = await relayCommands.localNetworkAccess();
        if (!signal.aborted) { setAccess(next); setFailed(false); }
      } catch {
        if (!signal.aborted) { setAccess(null); setFailed(true); }
      } finally { pending = false; }
    };
    void refresh();
    const interval = window.setInterval(refresh, 15_000);
    window.addEventListener("focus", refresh, { signal });
    window.addEventListener("online", refresh, { signal });
    document.addEventListener("visibilitychange", refresh, { signal });
    return () => {
      controller.abort();
      window.clearInterval(interval);
    };
  }, [port, busy]);

  const update = async (enabled: boolean) => {
    if (!access || busy) return;
    if ((enabled || running) && !await confirm(t(enabled ? "network.enableConfirm" : "network.restartConfirm"), {
      title: t("network.title"), confirmLabel: t(enabled ? "common.enable" : "common.disable"),
    })) return;
    await perform("network-access", async () => {
      const next = await relayCommands.setLocalNetworkAccess(enabled, access.enabled);
      setAccess(next);
    }, "feedback.saved", { backgroundRefresh: true });
  };
  const copyKey = async (rotate = false) => {
    if (rotate && !await confirm(t("network.rotateConfirm"), { title: t("network.key"), danger: true, confirmLabel: t("gateway.regenerateApiKey") })) return;
    await perform("network-key", async () => {
      await copyText(await (rotate ? relayCommands.rotateLocalGatewayApiKey : relayCommands.revealLocalGatewayApiKey)(true));
    }, "feedback.copied", { skipRefresh: !rotate, backgroundRefresh: rotate });
  };

  return <section className="gateway-api-section gateway-network-access" aria-label={t("network.title")}>
    <div className="gateway-api-section-heading gateway-network-heading">
      <Network aria-hidden />
      <div className="relay-toggle-setting">
        <label htmlFor={controlId}>
          <strong>{t("network.title")}</strong>
          <span id={`${controlId}-description`}>{t(failed ? "network.loadFailed" : !access ? "common.loading" : access.enabled ? "network.sharedHint" : "network.localHint")}</span>
        </label>
        <ToggleSwitch id={controlId} label={t("network.title")} aria-describedby={`${controlId}-description`}
          checked={access?.enabled ?? false} disabled={!access || Boolean(busy)} onChange={(enabled) => void update(enabled)} />
      </div>
    </div>
    {access?.enabled ? <div className="gateway-network-details">
      <div className="gateway-api-fields">
        <div className="gateway-api-field">
          <h4><Link2 aria-hidden />{t("network.address")}</h4>
          {access.addresses.length ? access.addresses.map(({ interface: name, baseUrl }) => <div className="gateway-api-value" key={`${name}:${baseUrl}`}>
            <div className="gateway-network-address"><small>{name}</small><code className="gateway-api-address">{baseUrl}</code></div>
            <CopyButton value={baseUrl} label={t("gateway.copyEndpoint")} />
          </div>) : <div className="gateway-api-value"><p>{t("network.addressUnavailable")}</p></div>}
        </div>
        <div className="gateway-api-field">
          <h4><KeyRound aria-hidden />{t("network.key")}</h4>
          <div className="gateway-api-value">
            <span className="gateway-api-secret" aria-label={t("gateway.keyHidden")}>•••• •••• ••••</span>
            <div className="gateway-api-field-actions">
              <IconButton label={t("network.copyKey")} icon={<Copy aria-hidden />} disabled={Boolean(busy)} onClick={() => void copyKey()} />
              <ActionMenu label={t("gateway.apiKeyActions")}>
                <ActionMenuItem icon={<RefreshCw aria-hidden />} disabled={Boolean(busy)} onClick={() => void copyKey(true)}>{t("gateway.regenerateApiKey")}</ActionMenuItem>
              </ActionMenu>
            </div>
          </div>
        </div>
      </div>
      <p className="gateway-network-security">{t("network.securityHint")}</p>
    </div> : null}
  </section>;
}
