import { Wallet } from "lucide-react";
import { FormEvent } from "react";
import { useTranslation } from "react-i18next";
import type { PreparedTopUpAmount } from "../tauri";

const QUICK_AMOUNTS = [10, 25, 50, 100];

type TopUpPanelProps = {
  disabled: boolean;
  error: boolean;
  loading: boolean;
  amount: string;
  preparedAmount: PreparedTopUpAmount;
  onAmountChange: (value: string) => void;
  onTopUp: () => Promise<void>;
};

export function TopUpPanel({ disabled, error, loading, amount, preparedAmount, onAmountChange, onTopUp }: TopUpPanelProps) {
  const { t } = useTranslation();

  const canSubmit = !disabled && !loading && preparedAmount.valid;

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!canSubmit) return;
    await onTopUp();
  }

  if (disabled) {
    return (
      <section className="top-up-panel compact" aria-label={t("topUp.label")}>
        <Wallet aria-hidden />
        <span>{t("topUp.saveKeyFirst")}</span>
      </section>
    );
  }

  return (
    <form className="top-up-panel" aria-label={t("topUp.label")} onSubmit={submit}>
      <div className="top-up-main">
        <div className="top-up-field">
          <Wallet aria-hidden />
          <span>$</span>
          <input
            value={amount}
            onChange={(event) => onAmountChange(event.target.value)}
            inputMode="decimal"
            disabled={disabled || loading}
            aria-label={t("topUp.amount")}
          />
        </div>

        <div className="top-up-presets">
          {QUICK_AMOUNTS.map((value) => (
            <button
              className={preparedAmount.amountUsd === value ? "selected" : ""}
              key={value}
              type="button"
              disabled={disabled || loading}
              onClick={() => onAmountChange(String(value))}
            >
              ${value}
            </button>
          ))}
        </div>

        <button className="top-up-submit" type="submit" disabled={!canSubmit}>
          {loading ? t("topUp.opening") : t("topUp.openBot")}
        </button>
      </div>

      {error ? <span className="top-up-hint error-text">{t("topUp.failed")}</span> : null}
    </form>
  );
}
