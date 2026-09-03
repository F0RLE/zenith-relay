const MAX_NUMBER_FORMATTERS = 32;
const numberFormatters = new Map<string, Intl.NumberFormat>();

type FormatterOptions = Intl.NumberFormatOptions;

function formatterKey(locale: string, options: FormatterOptions) {
  const definedOptions = Object.entries(options)
    .filter((entry): entry is [string, FormatterOptions[keyof FormatterOptions]] => entry[1] !== undefined)
    .sort(([left], [right]) => left < right ? -1 : left > right ? 1 : 0);
  return `${locale}\u0000${JSON.stringify(definedOptions)}`;
}

/** Reuses the small set of number formatters used by frequently refreshed views. */
export function getNumberFormatter(locale: string, options: FormatterOptions = {}) {
  const key = formatterKey(locale, options);
  const cached = numberFormatters.get(key);
  if (cached) {
    numberFormatters.delete(key);
    numberFormatters.set(key, cached);
    return cached;
  }
  const formatter = new Intl.NumberFormat(locale, options);
  numberFormatters.set(key, formatter);
  while (numberFormatters.size > MAX_NUMBER_FORMATTERS) {
    const oldestKey = numberFormatters.keys().next().value;
    if (oldestKey === undefined) break;
    numberFormatters.delete(oldestKey);
  }
  return formatter;
}

export function formatNumber(value: number, locale: string, options: FormatterOptions = {}) {
  return getNumberFormatter(locale, options).format(value);
}
