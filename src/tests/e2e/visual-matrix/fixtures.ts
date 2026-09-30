import { expect, type Page } from "../../bun-playwright";

export const modes = ["local", "remote", "zenith"] as const;
export const themes = ["light", "dark"] as const;
export const locales = ["en", "ru"] as const;
export const viewports = [{ width: 1160, height: 760 }, { width: 840, height: 560 }] as const;
export const TITLE_BAR_HEIGHT = 36;

export async function expectTopLevelEmptyCentered(page: Page) {
  const tabs = page.locator(".relay-page > .relay-tabs");
  const [pageBox, headerBox, tabCount, emptyBox, paddingBottom] = await Promise.all([
    page.locator(".relay-page").boundingBox(),
    page.locator(".relay-page-header").boundingBox(),
    tabs.count(),
    page.locator(".gateway-empty-tab-panel > .relay-empty, .relay-page > .relay-empty").boundingBox(),
    page.locator(".relay-page").evaluate((element) => Number.parseFloat(getComputedStyle(element).paddingBottom)),
  ]);
  const tabsBox = tabCount ? await tabs.boundingBox() : null;
  expect(pageBox).not.toBeNull();
  expect(headerBox).not.toBeNull();
  expect(emptyBox).not.toBeNull();
  const contentTop = tabsBox ? tabsBox.y + tabsBox.height : headerBox!.y + headerBox!.height;
  const availableCenter = (contentTop + pageBox!.y + pageBox!.height - paddingBottom) / 2;
  expect(Math.abs(emptyBox!.y + emptyBox!.height / 2 - availableCenter)).toBeLessThanOrEqual(2);
}
