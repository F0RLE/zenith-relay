import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import type { OAuthClientKind } from "../api/types";
import { useConfirm } from "../components/ui/confirm";
import { readRelayPreference, RELAY_STORAGE_KEYS } from "../state/relayPreferences";

type PoolAccount = { oauthClientKind?: OAuthClientKind; inPool?: boolean };

export function usePoolAccountWarning() {
  const { t } = useTranslation();
  const confirm = useConfirm();
  return useCallback(async (accounts: readonly PoolAccount[], bypass = false) => {
    if (bypass || !accounts.some((account) => !account.inPool && account.oauthClientKind !== "excel_bps")
      || readRelayPreference(RELAY_STORAGE_KEYS.hideChatgptPoolWarning, "0") === "1") return true;
    return confirm(t("accounts.poolWarning.message"), {
      title: t("accounts.poolWarning.title"),
      confirmLabel: t("accounts.poolWarning.continue"),
      cancelLabel: t("accounts.poolWarning.skip"),
      remember: { key: RELAY_STORAGE_KEYS.hideChatgptPoolWarning, label: t("accounts.poolWarning.remember") },
    });
  }, [confirm, t]);
}
