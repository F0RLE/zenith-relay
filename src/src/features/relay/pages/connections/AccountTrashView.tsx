import { useCallback, useEffect, useRef, useState } from "react";
import { RefreshCw, RotateCcw, Trash2, UserRound, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { TrashedAccountSummary } from "../../api/types";
import { Button, EmptyState, IconButton, StatusIcon, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { toggleSelectedId } from "../../state/selectionModel";
import { AccountCardLayout } from "./AccountCard";
import { NoResults } from "./connectionHelpers";

export function AccountTrashView() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const confirm = useConfirm();
  const [accounts, setAccounts] = useState<TrashedAccountSummary[]>([]);
  const [selected, setSelected] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const requestRevision = useRef(0);
  const mutationPending = useRef(false);
  const currentBusy = useRef(busy);
  currentBusy.current = busy;
  const targetId = mode === "remote" ? runtime?.runtimeTarget.serverId : "local";

  const load = useCallback(async () => {
    const revision = ++requestRevision.current;
    setLoading(true);
    setLoadFailed(false);
    try {
      const entries = await (mode === "local" ? relayCommands.accountTrash() : relayCommands.remoteAccountTrash());
      if (revision !== requestRevision.current) return;
      setAccounts(entries);
      setSelected((previous) => previous.filter((accountId) => entries.some((entry) => entry.id === accountId)));
    } catch {
      if (revision === requestRevision.current) setLoadFailed(true);
    } finally {
      if (revision === requestRevision.current) setLoading(false);
    }
  }, [mode, targetId]);

  useEffect(() => {
    setAccounts([]);
    setSelected([]);
    setQuery("");
    void load();
    return () => { requestRevision.current += 1; };
  }, [load]);

  const mutate = async (accountIds: string[], permanently: boolean) => {
    if (busy || mutationPending.current || !accountIds.length) return;
    mutationPending.current = true;
    const revision = requestRevision.current;
    try {
      if (permanently && !await confirm(t("trash.permanentConfirm", { count: accountIds.length }), {
        title: t("trash.deletePermanently"), confirmLabel: t("trash.deletePermanently"), danger: true,
      })) return;
      if (revision !== requestRevision.current || currentBusy.current) return;
      await perform(permanently ? "trash-purge" : "trash-restore", async () => {
        if (mode === "local") {
          if (permanently) await relayCommands.purgeAccountTrash(accountIds);
          else await relayCommands.restoreAccountTrash(accountIds);
        } else {
          for (const accountId of accountIds) {
            await relayCommands.remoteAction({
              type: permanently ? "purge_trashed_account" : "restore_trashed_account",
              id: accountId,
            });
          }
        }
      }, permanently ? "feedback.deleted" : "trash.restored", { backgroundRefresh: true });
      if (revision === requestRevision.current) await load();
    } finally {
      mutationPending.current = false;
    }
  };

  const visibleAccounts = accounts.filter((entry) => entry.label.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()));
  const toggleSelected = (accountId: string) => setSelected((selectedIds) => toggleSelectedId(selectedIds, accountId));
  const disabled = Boolean(busy) || loading;

  return <div className="account-trash-view">
    <div className="account-trash-toolbar">
      <div className="inline-actions">
        <input type="checkbox" aria-label={t("trash.selectAll")} disabled={!visibleAccounts.length || disabled}
          checked={Boolean(visibleAccounts.length) && visibleAccounts.every((entry) => selected.includes(entry.id))}
          onChange={(event) => setSelected(event.target.checked ? visibleAccounts.map((entry) => entry.id) : [])} />
        <label className="search-field">
          <span className="sr-only">{t("common.search")}</span>
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("common.search")} />
        </label>
      </div>
      <div className="inline-actions">
        {selected.length ? <>
          <span>{t("trash.selected", { count: selected.length })}</span>
          <Button variant="secondary" icon={<RotateCcw aria-hidden />} disabled={disabled} onClick={() => void mutate([...selected], false)}>{t("trash.restore")}</Button>
          <Button variant="danger" icon={<Trash2 aria-hidden />} disabled={disabled} onClick={() => void mutate([...selected], true)}>{t("trash.deletePermanently")}</Button>
          <IconButton label={t("accounts.clearSelection")} icon={<X aria-hidden />} disabled={disabled} onClick={() => setSelected([])} />
        </> : null}
        <IconButton label={t("common.refresh")} icon={<RefreshCw aria-hidden />} busy={loading} disabled={Boolean(busy)} onClick={() => void load()} />
      </div>
    </div>
    {loading ? <p role="status">{t("common.working")}</p>
      : loadFailed ? <EmptyState title={t("trash.loadFailed")} description={t("trash.retryHint")} action={<Button variant="secondary" onClick={() => void load()}>{t("common.refresh")}</Button>} />
      : !accounts.length ? <EmptyState title={t("trash.emptyTitle")} description={t("trash.emptyDescription")} />
      : !visibleAccounts.length ? <NoResults />
      : <div className="account-list" role="list" aria-label={t("trash.title")}>{visibleAccounts.map((entry) =>
        <AccountCardLayout
          key={entry.id}
          label={entry.label}
          selected={selected.includes(entry.id)}
          selectionDisabled={disabled}
          onToggleSelected={() => toggleSelected(entry.id)}
          icon={<StatusIcon className="account-kind-icon" status="disabled" label={t("trash.title")}><UserRound aria-hidden /></StatusIcon>}
          details={<small>{t("trash.deletedAt", { date: new Date(entry.trashedAtMs).toLocaleString(i18n.resolvedLanguage ?? i18n.language) })}</small>}
          actions={<>
            <IconButton label={t("trash.restore")} icon={<RotateCcw aria-hidden />} disabled={disabled} onClick={() => void mutate([entry.id], false)} />
            <IconButton className="danger" label={t("trash.deletePermanently")} icon={<Trash2 aria-hidden />} disabled={disabled} onClick={() => void mutate([entry.id], true)} />
          </>}
        />
      )}</div>}
  </div>;
}
