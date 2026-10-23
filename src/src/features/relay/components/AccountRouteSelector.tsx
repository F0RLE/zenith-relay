import { useState } from "react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import type { AccountSummary, OAuthClientKind } from "../api/types";
import { accountRouteConnections } from "../accountRoutes";
import { useAccountSignIn } from "../hooks/useAccountSignIn";
import { useRelayState } from "../state/RelayStateProvider";
import { captureOperationResult } from "../state/relayOperationModel";
import { OptionMenu } from "./Ui";

const kinds: OAuthClientKind[] = ["codex", "siwc", "excel_bps"];
const names: Record<OAuthClientKind, string> = { codex: "Codex", siwc: "SIWC", excel_bps: "Excel / BPS" };

export function AccountRouteSelector({ account }: { account: AccountSummary }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [saving, setSaving] = useState(false);
  const signIn = useAccountSignIn();
  const accounts = runtime?.accounts ?? [];
  const connections = accountRouteConnections(account, accounts);
  const active = runtime?.gateway.routingOrder?.some((candidate) =>
    connections.some((connection) => connection.id === candidate.candidateId) && ((candidate.activeRequestCount ?? 0) > 0 || candidate.inFlight > 0),
  ) ?? false;
  const disabled = Boolean(busy) || saving || active || account.recoveryRequired || !account.secretAvailable;
  const selectedKind = account.oauthClientKind ?? "codex";

  const select = async (connectionId: string) => {
    if (disabled) return;
    setSaving(true);
    try {
      await captureOperationResult(
        (work) => perform(`account-route-${account.id}`, work, undefined, { backgroundRefresh: true }),
        () => relayCommands.selectAccountRoute(account.id, connectionId),
      );
    } finally { setSaving(false); }
  };
  const choose = (value: string) => {
    const kind = kinds.find((candidate) => candidate === value);
    if (disabled || !kind || kind === selectedKind) return;
    const existing = connections.find((connection) => (connection.oauthClientKind ?? "codex") === kind);
    if (existing) void select(existing.id);
    else signIn.startRoute(account, kind, kind !== "siwc");
  };

  if (mode !== "local") return <span className="account-client-badge">{names[selectedKind]}</span>;
  return <>
    <OptionMenu
      className="account-route-selector"
      label={active ? t("accounts.routes.busy") : t("accounts.routes.select")}
      value={selectedKind}
      disabled={Boolean(disabled)}
      align="start"
      fitContent
      options={kinds.map((kind) => ({ value: kind, label: connections.some((connection) => (connection.oauthClientKind ?? "codex") === kind) ? names[kind] : `${names[kind]} · ${t("accounts.routes.connect")}`, shortLabel: names[kind] }))}
      onChange={choose}
    />
    {signIn.dialogs}
  </>;
}
