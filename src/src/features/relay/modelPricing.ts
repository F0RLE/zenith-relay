const MICRO_USD_PER_USD = 1_000_000;

const MAX_MODEL_PRICE_USD_PER_MILLION = 1_000_000;

export function formatEditableModelPrice(microUsd: number | null | undefined) {
  return microUsd == null ? "" : (microUsd / MICRO_USD_PER_USD).toFixed(6).replace(/\.?0+$/, "");
}

export function formatModelPricePlaceholder(microUsd: number | null | undefined) {
  return microUsd == null ? "—" : formatEditableModelPrice(microUsd);
}

export function parseEditableModelPrice(priceText: string) {
  const normalized = priceText.trim().replace(",", ".");
  if (!/^\d+(?:\.\d{0,6})?$/.test(normalized)) return null;
  const price = Number(normalized);
  return Number.isFinite(price) && price <= MAX_MODEL_PRICE_USD_PER_MILLION
    ? Math.round(price * MICRO_USD_PER_USD)
    : null;
}

export function parseOptionalEditableModelPrice(priceText: string) {
  return priceText.trim() === "" ? null : parseEditableModelPrice(priceText) ?? undefined;
}
