import { useEffect, useRef, useState } from "react";
import type { MouseEvent } from "react";
import { Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { RelayMode } from "../../api/types";
import { ContextMenu } from "../../components/ContextMenu";
import { ActionMenuItem, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { expandAccountRouteIds } from "../../accountRoutes";

type DeleteMenuState = {
  x: number;
  y: number;
  trigger: HTMLElement;
  onDelete: () => void;
};

export function useConnectionAccountRemoval() {
  const { t } = useTranslation();
  const confirm = useConfirm();
  const { mode, runtime, busy, perform } = useRelayState();
  const pending = useRef(false);
  const mounted = useRef(true);
  const target = mode;
  const currentTarget = useRef(target);
  const targetRevision = useRef(0);
  const currentBusy = useRef(busy);
  currentBusy.current = busy;
  const [deleteMenu, setDeleteMenu] = useState<DeleteMenuState | null>(null);
  if (currentTarget.current !== target) {
    currentTarget.current = target;
    targetRevision.current += 1;
  }
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => { setDeleteMenu(null); }, [target, busy]);
  const remove = async (accountIds: string[], permanently: boolean, operation: string, localRecovery = false) => {
    if (target !== currentTarget.current || !accountIds.length || pending.current || currentBusy.current) return false;
    if (mode !== "local") return false;
    accountIds = expandAccountRouteIds(accountIds, runtime?.accounts ?? []);
    pending.current = true;
    const revision = targetRevision.current;
    try {
      if (permanently && !await confirm(t(localRecovery ? "trash.permanentRecoveryConfirm" : "trash.permanentConfirm", { count: accountIds.length }), {
        title: t("trash.deletePermanently"), confirmLabel: t("trash.deletePermanently"), danger: true,
      })) return false;
      if (!mounted.current || revision !== targetRevision.current || currentBusy.current) return false;
      return await perform(operation, () => removeConnectionAccounts(mode, accountIds, permanently),
        permanently ? "feedback.deleted" : "trash.moved", { backgroundRefresh: true });
    } finally {
      pending.current = false;
    }
  };
  const openDeleteMenu = (event: MouseEvent<HTMLButtonElement>, onDelete: () => void) => {
    event.preventDefault();
    event.stopPropagation();
    if (pending.current || currentBusy.current) return;
    const button = event.currentTarget;
    const bounds = button.getBoundingClientRect();
    const actionMenu = button.closest("details");
    const trigger = actionMenu?.querySelector("summary") ?? button;
    if (actionMenu) actionMenu.open = false;
    setDeleteMenu({
      x: event.clientX || bounds.left + 8,
      y: event.clientY || bounds.bottom,
      trigger,
      onDelete,
    });
  };
  const closeDeleteMenu = (restoreFocus = false) => {
    if (restoreFocus) deleteMenu?.trigger.focus({ preventScroll: true });
    setDeleteMenu(null);
  };
  const permanentDeleteMenu = deleteMenu ? (
    <ContextMenu position={deleteMenu} onClose={closeDeleteMenu}>
      <ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => {
        closeDeleteMenu(true);
        deleteMenu.onDelete();
      }}>
        {t("trash.deletePermanently")}
      </ActionMenuItem>
    </ContextMenu>
  ) : null;
  return { remove, openDeleteMenu, permanentDeleteMenu };
}

export async function removeConnectionAccounts(mode: RelayMode, accountIds: string[], permanently: boolean) {
  if (mode !== "local") throw new Error("Account removal requires the local pool");
  if (!permanently) return relayCommands.trashAccounts(accountIds);
  if (accountIds.length === 1 && accountIds[0]) return relayCommands.deleteAccount(accountIds[0]);
  return relayCommands.deleteAccounts(accountIds);
}
