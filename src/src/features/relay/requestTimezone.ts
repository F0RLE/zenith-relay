import type { TFunction } from "i18next";

export const DEFAULT_REQUEST_TIMEZONE = "America/New_York";

const CITY_KEYS: Record<string, string> = {
  "America/New_York": "newYork",
  "America/Chicago": "chicago",
  "America/Denver": "denver",
  "America/Los_Angeles": "losAngeles",
  "Europe/Minsk": "minsk",
  "Europe/London": "london",
  "Europe/Berlin": "berlin",
  "Europe/Moscow": "moscow",
  "Asia/Singapore": "singapore",
  "Asia/Tokyo": "tokyo",
  "Asia/Dubai": "dubai",
  "UTC": "utc",
};

type TimezoneRegion = "america" | "europe" | "asia" | "other";

function timezoneRegion(zone: string): TimezoneRegion {
  if (zone.startsWith("America/") || zone.startsWith("US/") || zone.startsWith("Canada/")) return "america";
  if (zone.startsWith("Europe/")) return "europe";
  if (zone.startsWith("Asia/")) return "asia";
  return "other";
}

function timezoneCity(zone: string, t: TFunction): string {
  const key = CITY_KEYS[zone];
  return key ? t(`gateway.timezone.cities.${key}`) : (zone.split("/").at(-1) ?? zone).replaceAll("_", " ");
}

function formatUtcOffset(minutes: number): string {
  if (minutes === 0) return "UTC";
  const sign = minutes < 0 ? "-" : "+";
  const absolute = Math.abs(minutes);
  const hours = Math.floor(absolute / 60).toString().padStart(2, "0");
  const remainder = (absolute % 60).toString().padStart(2, "0");
  return `UTC${sign}${hours}:${remainder}`;
}

function fixedOffsetMinutes(zone: string): number | null {
  const match = zone.match(/^Etc\/GMT([+-])(\d{1,2})$/);
  if (!match) return zone === "Etc/GMT" ? 0 : null;
  const minutes = Number(match[2]) * 60;
  // POSIX Etc/GMT names use the inverse sign: GMT+3 is UTC-03:00.
  return match[1] === "+" ? -minutes : minutes;
}

export function timezoneOffsetMinutes(zone: string, date: Date): number | null {
  try {
    const offset = new Intl.DateTimeFormat("en", { timeZone: zone, timeZoneName: "longOffset" })
      .formatToParts(date).find((part) => part.type === "timeZoneName")?.value;
    if (!offset) return fixedOffsetMinutes(zone);
    if (offset === "GMT" || offset === "GMT+00:00") return 0;
    const match = offset.match(/^GMT([+-])(\d{1,2})(?::?(\d{2}))?$/);
    if (!match) return null;
    const minutes = Number(match[2]) * 60 + Number(match[3] ?? 0);
    return match[1] === "-" ? -minutes : minutes;
  } catch {
    // The host's catalog can be newer than the WebView's timezone database.
    // Keep that valid choice available and retain deterministic fixed offsets.
    return fixedOffsetMinutes(zone);
  }
}

export function timezoneOffset(zone: string, date: Date): string {
  const minutes = timezoneOffsetMinutes(zone, date);
  return minutes == null ? "" : formatUtcOffset(minutes);
}

function isFixedOffsetZone(zone: string): boolean {
  return /^Etc\/GMT(?:[+-]\d{1,2})?$/.test(zone);
}

function localizedGenericName(zone: string, date: Date, locale: string): string {
  try {
    const name = new Intl.DateTimeFormat(locale, { timeZone: zone, timeZoneName: "shortGeneric" })
      .formatToParts(date).find((part) => part.type === "timeZoneName")?.value?.trim();
    if (!name || /^(?:GMT|UTC)(?:[+-]\d{1,2}(?::?\d{2})?)?$/i.test(name)) return "";
    return name;
  } catch {
    return "";
  }
}

export function timezoneDisplayName(
  zone: string,
  date: Date,
  t: TFunction,
  locale = "en",
  offsetMinutes = timezoneOffsetMinutes(zone, date),
): string {
  if (isFixedOffsetZone(zone) && offsetMinutes != null) return formatUtcOffset(offsetMinutes);
  const city = timezoneCity(zone, t);
  if (CITY_KEYS[zone]) return city;
  return localizedGenericName(zone, date, locale) || city;
}

export function timezoneSearchText(value: string): string {
  return value.toLocaleLowerCase().replaceAll("_", " ").replaceAll("ё", "е").replaceAll("−", "-");
}

function canonicalTimezone(zone: string): string {
  try {
    return new Intl.DateTimeFormat("en", { timeZone: zone }).resolvedOptions().timeZone;
  } catch {
    return zone;
  }
}

export function timezoneOptions(zones: readonly string[], date: Date, t: TFunction, locale = "en", selectedZone = "") {
  const groups = new Map<string, { zone: string; aliases: string[] }>();
  for (const zone of new Set(zones)) {
    const canonical = canonicalTimezone(zone);
    const group = groups.get(canonical);
    if (!group) {
      groups.set(canonical, { zone, aliases: [zone] });
    } else {
      group.aliases.push(zone);
      // Preserve an existing selection; otherwise prefer the primary IANA name.
      if (zone === selectedZone || (group.zone !== selectedZone && zone === canonical)) group.zone = zone;
    }
  }
  return [...groups.values()].map(({ zone, aliases }) => {
    const offsetMinutes = timezoneOffsetMinutes(zone, date);
    const offset = offsetMinutes == null ? "" : formatUtcOffset(offsetMinutes);
    const city = timezoneDisplayName(zone, date, t, locale, offsetMinutes);
    const region = timezoneRegion(zone);
    return {
      zone,
      city,
      region,
      offset,
      offsetMinutes,
      search: timezoneSearchText(`${aliases.join(" ")} ${city} ${offset} ${t(`gateway.timezone.regions.${region}`)}`),
    };
  }).sort((left, right) => {
    const leftOffset = left.offsetMinutes ?? Number.POSITIVE_INFINITY;
    const rightOffset = right.offsetMinutes ?? Number.POSITIVE_INFINITY;
    return leftOffset - rightOffset
      || left.city.localeCompare(right.city, locale, { sensitivity: "base" })
      || left.zone.localeCompare(right.zone);
  });
}
