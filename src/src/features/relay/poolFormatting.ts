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

export function sortReasoningEfforts(levels: string[]) {
  const rank = (level: string) => {
    switch (level.replace(/-/g, "_")) {
      case "none": return 0;
      case "minimal": return 1;
      case "low": return 2;
      case "medium": return 3;
      case "high": return 4;
      case "xhigh":
      case "very_high":
      case "extra_high": return 5;
      case "max": return 6;
      case "ultra": return 7;
      default: return 8;
    }
  };
  return levels
    .map((level, index) => ({ level: normalizeReasoningEffort(level), index }))
    .filter(({ level }) => Boolean(level))
    .filter(({ level }, index, values) => values.findIndex((candidate) => candidate.level === level) === index)
    .sort((left, right) => rank(left.level) - rank(right.level) || left.index - right.index)
    .map(({ level }) => level);
}

function normalizeReasoningEffort(value: string) {
  return value.trim().toLowerCase();
}
