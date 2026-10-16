import { useTranslation } from "react-i18next";
import { OptionMenu } from "../../components/Ui";
import { usePendingFlag } from "../../state/usePendingFlag";
import { useRequestTimezone } from "../../hooks/useRequestTimezone";

export function RequestTimezoneControl() {
  const { t } = useTranslation();
  const { supported, policy, busy, persist } = useRequestTimezone();
  const { checked, saving, select } = usePendingFlag(policy.enabled);
  if (!supported) return null;
  return <div className="integration-account-policy request-timezone-policy">
    <div className="request-timezone-default">
      <div>
        <strong>{t("gateway.timezone.title")}</strong>
        <p>{t("gateway.timezone.globalDescription")}</p>
      </div>
      <OptionMenu label={t("gateway.timezone.title")} value={checked ? "account" : "proxy"}
        options={[
          { value: "proxy", label: t("gateway.timezone.sourceProxy") },
          { value: "account", label: t("gateway.timezone.sourceAccount") },
        ]} disabled={Boolean(busy) || saving} onChange={(source) => {
          const enabled = source === "account";
          select(enabled, () => persist({ ...policy, enabled }));
        }} />
    </div>
  </div>;
}
