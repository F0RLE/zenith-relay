import { describe, expect, test } from "bun:test";
import { calendarDate, calendarDays, displayCalendarDate, moveCalendarDays, moveCalendarMonths, parseCalendarInput } from "../../src/components/calendar-date";

describe("calendar-only expiry dates", () => {
  test("manual input preserves ISO dates and accepts localized or mobile numeric entry", () => {
    expect(parseCalendarInput("2028-02-29")).toBe("2028-02-29");
    expect(parseCalendarInput(" 29.2.2028 ", "ru")).toBe("2028-02-29");
    expect(parseCalendarInput("29022028", "ru")).toBe("2028-02-29");
    expect(displayCalendarDate("2028-02-29", "ru")).toBe("29.02.2028");
    expect(parseCalendarInput(" ")).toBe("");
    expect(displayCalendarDate("")).toBe("");
  });
  test("impossible dates never silently roll into another month", () => {
    for (const value of ["2027-02-29", "2028-02-30", "31.04.2028", "2028-00-01", "2028-13-01", "0000-01-01", "31.12.", "invalid"])
      expect(parseCalendarInput(value)).toBeNull();
    expect(parseCalendarInput("2000-02-29")).toBe("2000-02-29");
    expect(parseCalendarInput("1900-02-29")).toBeNull();
    expect(parseCalendarInput("0099-12-31")).toBe("0099-12-31");
  });
  test("month and year movement clamps the day without changing timezone", () => {
    expect(moveCalendarMonths("2028-01-31", 1)).toBe("2028-02-29");
    expect(moveCalendarMonths("2028-02-29", 12)).toBe("2029-02-28");
    expect(moveCalendarMonths("2028-01-31", -1)).toBe("2027-12-31");
    expect(moveCalendarDays("2028-02-28", 1)).toBe("2028-02-29");
    expect(moveCalendarDays("2028-02-29", 1)).toBe("2028-03-01");
    expect(moveCalendarDays("2028-12-31", 1)).toBe("2029-01-01");
    expect(moveCalendarDays("2026-03-08", 1)).toBe("2026-03-09");
    expect(moveCalendarDays("2026-11-01", -1)).toBe("2026-10-31");
  });
  test("six consistent calendar rows start Monday and include neighboring months", () => {
    const days = calendarDays("2028-02-29", "ru");
    expect(days).toHaveLength(42);
    expect(days[0]).toBe("2028-01-31");
    expect(days.at(-1)).toBe("2028-03-12");
    expect(calendarDate(days[0]!).getUTCDay()).toBe(1);
    expect(new Set(days).size).toBe(42);
    expect(days).toContain("2028-02-29");
  });
});
