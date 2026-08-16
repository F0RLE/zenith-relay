export function formatApiEquivalent(microUsd: number, locale: string) {
  return `≈${new Intl.NumberFormat(locale, {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 6,
  }).format(microUsd / 1_000_000)}`;
}

export function formatProviderMicroUsd(value: number, locale: string) {
  return new Intl.NumberFormat(locale, {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(value / 1_000_000);
}

export function formatAccountValueMicroUsd(value: number, locale: string, approximate = false) {
  const formatted = new Intl.NumberFormat(locale, {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 0,
    maximumFractionDigits: 2,
  }).format(value / 1_000_000);
  return `${approximate ? "≈" : ""}${formatted}`;
}

export function formatModelPrice(microUsd: number, locale: string) {
  return `$${new Intl.NumberFormat(locale, { maximumFractionDigits: 6 }).format(microUsd / 1_000_000)}`;
}

export function formatReasoningEffort(effort: string) {
  return effort.replace(/[_-]+/g, " ").replace(/\b\w/g, (letter) => letter.toUpperCase());
}
