import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import type { AccountSummary, OAuthClientKind } from "../api/types";
import { accountRouteConnections } from "../accountRoutes";
import { useAccountSignIn } from "../hooks/useAccountSignIn";
import { useRelayState } from "../state/RelayStateProvider";
import { OptionMenu } from "./Ui";

const names: Record<OAuthClientKind, string> = { codex: "Codex", siwc: "SIWC", excel_bps: "Excel / BPS" };
const kinds = Object.keys(names) as OAuthClientKind[];

export function AccountRouteSelector({ account }: { account: AccountSummary }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const signIn = useAccountSignIn();
  const accounts = runtime?.accounts ?? [];
  const connections = accountRouteConnections(account, accounts);
  const connectionFor = (kind: OAuthClientKind) => connections.find((connection) => (connection.oauthClientKind ?? "codex") === kind);
  const active = runtime?.gateway.routingOrder?.some((candidate) =>
    connections.some((connection) => connection.id === candidate.candidateId) && ((candidate.activeRequestCount ?? 0) > 0 || candidate.inFlight > 0),
  );
  const disabled = Boolean(busy) || active || account.recoveryRequired || !account.secretAvailable;
  const selectedKind = account.oauthClientKind ?? "codex";

  const choose = async (value: string) => {
    const kind = kinds.find((candidate) => candidate === value);
    if (disabled || !kind || kind === selectedKind) return;
    const existing = connectionFor(kind);
    let saved = existing?.id;
    const selected = await perform(
      `account-route-${account.id}`,
      async () => {
        saved ??= (await relayCommands.accountRouteCandidates(account.id, kind))[0];
        if (saved) await relayCommands.selectAccountRoute(account.id, saved, !existing);
      },
      undefined, { backgroundRefresh: true },
    );
    if (selected && !saved) signIn.startRoute(account, kind, kind !== "siwc");
  };

  if (mode !== "local") return <span className="account-client-badge">{names[selectedKind]}</span>;
  return <>
    <OptionMenu
      className="account-route-selector"
      label={t(active ? "accounts.routes.busy" : "accounts.routes.select")}
      value={selectedKind}
      disabled={disabled}
      align="start"
      fitContent
      options={kinds.map((kind) => ({ value: kind, label: connectionFor(kind) ? names[kind] : `${names[kind]} · ${t("accounts.routes.connect")}`, shortLabel: names[kind] }))}
      onChange={choose}
    />
    {signIn.dialogs}
  </>;
}
