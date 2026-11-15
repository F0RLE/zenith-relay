import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import type { AccountSummary, OAuthClientKind } from "../api/types";
import { accountRouteConnections, preferredAccountRouteConnection } from "../accountRoutes";
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
  const connectionFor = (kind: OAuthClientKind) => preferredAccountRouteConnection(connections, kind);
  const active = runtime?.gateway.routingOrder?.some((candidate) =>
    connections.some((connection) => connection.id === candidate.candidateId) && ((candidate.activeRequestCount ?? 0) > 0 || candidate.inFlight > 0),
  );
  const disabled = Boolean(busy || active);
  const canAttach = account.secretAvailable && !account.recoveryRequired;
  const selectedKind = account.oauthClientKind;

  const choose = async (value: string) => {
    const kind = kinds.find((candidate) => candidate === value);
    if (disabled || !kind || kind === selectedKind) return;
    const existing = connectionFor(kind);
    if (!existing && !canAttach) return;
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

  if (mode !== "local") return <span className="account-client-badge">{selectedKind ? names[selectedKind] : t("common.unknown")}</span>;
  return <>
    <OptionMenu
      className="account-route-selector"
      label={t(active ? "accounts.routes.busy" : "accounts.routes.select")}
      value={selectedKind ?? "unknown"}
      disabled={disabled}
      align="start"
      fitContent
      options={[
        ...(selectedKind ? [] : [{ value: "unknown", label: t("common.unknown") }]),
        ...kinds.filter((kind) => kind === selectedKind || connectionFor(kind) || canAttach)
          .map((kind) => ({ value: kind, label: connectionFor(kind) ? names[kind] : `${names[kind]} · ${t("accounts.routes.connect")}`, shortLabel: names[kind] })),
      ]}
      onChange={choose}
    />
    {signIn.dialogs}
  </>;
}
