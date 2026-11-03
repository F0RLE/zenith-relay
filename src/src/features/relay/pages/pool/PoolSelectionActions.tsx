import { ListMinus, Loader2, Pause, Play, RefreshCw, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { refreshOneAccountQuota } from "../../accountQuotaRefresh";
import { IconButton, useConfirm } from "../../components/Ui";
import type { PoolMember } from "../../poolHelpers";
import { updatePoolMembership } from "../../poolMembership";
import { useRelayState } from "../../state/relayStateContext";
import { poolRefreshTargets } from "./poolSelectionModel";

export function PoolSelectionActions({ members, onClear, onRemoved, onRefreshSource }: {
  members: readonly PoolMember[];
  onClear: () => void;
  onRemoved: (members: readonly PoolMember[]) => void;
  onRefreshSource: (sourceId: string) => Promise<void>;
}) {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform, refresh } = useRelayState();
  const confirm = useConfirm();
  const disabled = Boolean(busy) || mode !== "local";
  const targets = poolRefreshTargets(members, runtime?.accounts ?? []);
  const run = (key: string, action: () => Promise<void>, successKey?: string) => perform(key, async () => {
    try {
      await action();
    } catch (cause) {
      // A batch can partially succeed. Show persisted state, retaining selection for retry.
      await refresh().catch(() => undefined);
      throw cause;
    }
  }, successKey, { backgroundRefresh: true });
  const setEnabled = async (enabled: boolean) => {
    if (disabled) return;
    await run("pool-selected-enabled", async () => {
      for (const member of members.filter((member) => member.enabled !== enabled)) {
        if (member.kind === "account") await relayCommands.setAccountEnabled(member.id, enabled);
        else await relayCommands.setSourceEnabled(member.id, enabled);
      }
    }, "feedback.saved");
  };
  const refreshSelected = async () => {
    if (disabled) return;
    await run("pool-selected-refresh", async () => {
      const results = await Promise.allSettled([
        ...targets.accountIds.map((id) => refreshOneAccountQuota(mode, id)),
        ...targets.sourceIds.map(onRefreshSource),
      ]);
      const failure = results.find((result) => result.status === "rejected");
      if (failure?.status === "rejected") throw failure.reason;
    });
  };
  const removeSelected = async () => {
    if (disabled || !await confirm(t("pool.removeSelectedConfirm", { count: members.length }), {
      danger: true, confirmLabel: t("pool.removeMember"),
    })) return;
    const ok = await run("pool-selected-remove", async () => {
      await updatePoolMembership(mode, {
        accountIds: members.filter((member) => member.kind === "account").map((member) => member.id),
        sourceIds: members.filter((member) => member.kind === "source").map((member) => member.id),
        inPool: false,
      });
    }, "feedback.saved");
    if (ok) onRemoved(members);
  };
  return <div className="pool-quota-actions pool-selection-actions">
    <IconButton label={t("pool.refreshSelected")} icon={busy === "pool-selected-refresh" ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />}
      disabled={disabled || !targets.accountIds.length && !targets.sourceIds.length} onClick={() => void refreshSelected()} />
    <IconButton label={t("common.enable")} icon={<Play aria-hidden />} disabled={disabled || members.every((member) => member.enabled)} onClick={() => void setEnabled(true)} />
    <IconButton label={t("common.disable")} icon={<Pause aria-hidden />} disabled={disabled || members.every((member) => !member.enabled)} onClick={() => void setEnabled(false)} />
    <IconButton label={t("pool.removeMember")} icon={<ListMinus aria-hidden />} disabled={disabled} onClick={() => void removeSelected()} />
    <IconButton label={t("accounts.clearSelection")} icon={<X aria-hidden />} disabled={Boolean(busy)} onClick={onClear} />
  </div>;
}
