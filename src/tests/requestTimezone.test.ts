import { describe, expect, test } from "bun:test";
import i18next from "i18next";
import { en } from "../src/i18n/locales/en";
import { ru } from "../src/i18n/locales/ru";
import {
  timezoneOffset,
  timezoneOffsetMinutes,
  timezoneOptions,
  timezoneSearchText,
} from "../src/features/relay/requestTimezone";

describe("account timezone presentation", () => {
  test("offsets follow the selected zone's daylight saving rules and fractional hours", () => {
    const winter = new Date("2026-01-15T12:00:00Z");
    const summer = new Date("2026-07-15T12:00:00Z");
    expect(timezoneOffset("America/New_York", winter)).toBe("UTC-05:00");
    expect(timezoneOffset("America/New_York", summer)).toBe("UTC-04:00");
    expect(timezoneOffset("Etc/GMT-3", summer)).toBe("UTC+03:00");
    expect(timezoneOffset("Asia/Kathmandu", summer)).toBe("UTC+05:45");
    expect(timezoneOffset("Etc/GMT+3", summer)).toBe("UTC-03:00");
    expect(timezoneOffset("UTC", summer)).toBe("UTC");
  });

  test("an unrecognized WebView timezone stays visible in the host catalog", async () => {
    const translations = i18next.createInstance();
    await translations.init({ lng: "en", resources: { en: { translation: en } } });
    const options = timezoneOptions(["Future/New_City"], new Date("2026-07-15T12:00:00Z"), translations.t);
    expect(options[0]).toMatchObject({ zone: "Future/New_City", city: "New City", offset: "" });
  });

  test("IANA id and UTC searches address the same authoritative choices", async () => {
    const translations = i18next.createInstance();
    await translations.init({ lng: "ru", resources: { ru: { translation: ru } } });
    const options = timezoneOptions(["Etc/GMT-3", "America/New_York", "UTC"], new Date("2026-07-15T12:00:00Z"), translations.t);
    const matches = (query: string) => options.filter((option) => option.search.includes(timezoneSearchText(query))).map((option) => option.zone);
    expect(matches("Etc/GMT-3")).toEqual(["Etc/GMT-3"]);
    expect(matches("new_york")).toEqual(["America/New_York"]);
    expect(matches("UTC−04:00")).toEqual(["America/New_York"]);
  });

  test("catalog entries are deduplicated and ordered by their real UTC offset", async () => {
    const translations = i18next.createInstance();
    await translations.init({ lng: "en", resources: { en: { translation: en } } });
    const summer = new Date("2026-07-15T12:00:00Z");
    const options = timezoneOptions(
      ["Etc/GMT-3", "Etc/GMT+3", "America/New_York", "America/Chicago", "Etc/GMT-14", "Etc/GMT-3"],
      summer,
      translations.t,
    );
    expect(options.map((option) => option.zone)).toEqual([
      "America/Chicago",
      "America/New_York",
      "Etc/GMT+3",
      "Etc/GMT-3",
      "Etc/GMT-14",
    ]);
    expect(options.map((option) => option.offsetMinutes)).toEqual([-300, -240, -180, 180, 840]);
    expect(timezoneOffsetMinutes("Etc/GMT+3", summer)).toBe(-180);
  });
});
