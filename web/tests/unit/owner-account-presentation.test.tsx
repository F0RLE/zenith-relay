import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { OwnerAccountBody } from "../../src/features/owner/OwnerAccountBody";
import { ownerAccountTotals, ownerAccountValue, ownerQuotaPercent, ownerQuotaWindows, ownerRemainingTime } from "../../src/features/owner/accountPresentation";
import { previewOwnerAccount } from "../../src/preview/ownerAccounts";
import { buildAccountValueProjection, formatAccountPayback, formatRemainingApiEquivalent } from "../../../src/src/features/relay/accountEconomics";
import { formatAccountValueMicroUsd } from "../../../src/src/features/relay/poolFormatting";
import { LocaleProvider } from "../../src/i18n/LocaleProvider";

const now = Date.parse("2026-10-20T14:32:00Z");
const body = (account = previewOwnerAccount(now, "normal"), nowMs = now) => renderToStaticMarkup(<LocaleProvider><OwnerAccountBody account={account} nowMs={nowMs} /></LocaleProvider>);

describe("Hosted account presentation reuses Desktop contracts", () => {
  test("totals keep unknowns distinct from observed zero and mark partial coverage", () => {
    expect(ownerAccountTotals([{ id: "missing" }], now)).toMatchObject({ used: "—", remaining: "—", payback: "—", usageCount: 0 });
    expect(ownerAccountTotals([], now)).toMatchObject({ used: "—", remaining: "—", payback: "—", accountCount: 0 });
    const account = previewOwnerAccount(now, "normal");
    account.apiEquivalent!.microUsd = 0;
    account.quota.directBalanceMicroUsd = 0;
    const total = ownerAccountTotals([{ id: "known", account }, { id: "missing" }], now);
    expect(total.used).toBe(formatAccountValueMicroUsd(0, "en", true));
    expect(total.remaining).toBe(formatRemainingApiEquivalent({ microUsd: 0, approximate: true }, "en"));
    expect(total.payback).toBe(formatAccountPayback(0, "en", true));
  });

  test("total payback uses matched purchase costs, never an average or unmatched usage", () => {
    const normal = previewOwnerAccount(now, "normal"), low = previewOwnerAccount(now, "low");
    const total = ownerAccountTotals([{ id: "a", account: normal }, { id: "b", account: low }], now);
    expect(total.used).toBe(formatAccountValueMicroUsd(100000000, "en"));
    expect(total.payback).toBe(formatAccountPayback(2, "en", true));
    expect(total.purchaseCount).toBe(1);
    low.purchaseCostMicroUsd = 30000000;
    expect(ownerAccountTotals([{ id: "a", account: normal }, { id: "b", account: low }], now).payback).toBe(formatAccountPayback(100 / 35, "en", false));
  });

  test("remaining totals deduplicate shared ledgers and do not renew expired estimates", () => {
    const first = previewOwnerAccount(now, "normal"), second = previewOwnerAccount(now, "low");
    second.quotaWindowUsage!.ledgerKey = first.quotaWindowUsage!.ledgerKey;
    first.quotaWindowUsage!.observedAtMs -= 2;
    second.quotaWindowUsage!.observedAtMs -= 1;
    const accounts = [{ id: "a", account: first }, { id: "b", account: second }];
    const total = ownerAccountTotals(accounts, now);
    expect(total.remainingCount).toBe(1);
    expect(total.remainingTotalCount).toBe(1);
    expect(total.remaining).toBe(ownerAccountValue(second, now).remaining);
    expect(ownerAccountTotals(accounts, now + 7 * 86400000).remaining).toBe("—");
  });

  test("uses the exact Desktop estimates, approximation and payback", () => {
    for (const variant of ["normal", "low"] as const) {
      const account = previewOwnerAccount(now, variant);
      const desktop = buildAccountValueProjection(account.apiEquivalent!, account.quota, account.quotaWindowUsage, account.purchaseCostMicroUsd, now);
      expect(ownerAccountValue(account, now)).toEqual({
        used: formatAccountValueMicroUsd(account.apiEquivalent!.microUsd, "en", desktop.approximate),
        remaining: formatRemainingApiEquivalent(desktop.remainingApiEquivalent, "en"),
        payback: formatAccountPayback(desktop.payback, "en", desktop.approximate),
        paid: desktop.payback != null && desktop.payback >= 1, direct: false,
      });
    }
  });

  test("missing usage is unknown, not zero; an explicit balance remains independent", () => {
    expect(ownerAccountValue(undefined, now)).toEqual({ used: "—", remaining: "—", payback: "—", paid: false, direct: false });
    const account = previewOwnerAccount(now, "normal");
    account.apiEquivalent = null;
    account.quota.directBalanceMicroUsd = 0;
    expect(ownerAccountValue(account, now)).toEqual({ used: "—", remaining: formatAccountValueMicroUsd(0, "en"), payback: "—", paid: false, direct: true });
    account.quota.directBalanceMicroUsd = 1230000;
    expect(ownerAccountValue(account, now).remaining).toBe(formatAccountValueMicroUsd(1230000, "en"));
  });

  test("zero or missing purchase cost has no payback; partial pricing stays approximate", () => {
    const account = previewOwnerAccount(now, "normal");
    for (const cost of [0, null]) {
      account.purchaseCostMicroUsd = cost;
      expect(ownerAccountValue(account, now).payback).toBe("—");
      expect(ownerAccountValue(account, now).paid).toBe(false);
    }
    account.purchaseCostMicroUsd = 5000000;
    account.apiEquivalent!.unpricedTokens = 42;
    expect(ownerAccountValue(account, now).used.startsWith("≈")).toBe(true);
    expect(ownerAccountValue(account, now).payback.startsWith("≈")).toBe(true);
  });

  test("expired calibration and windows do not infer new balances or quota resets", () => {
    const account = previewOwnerAccount(now, "normal");
    const later = now + 7 * 86400000;
    expect(ownerAccountValue(account, later).remaining).toBe("—");
    const html = body(account, later);
    expect(html).toContain("Awaiting refresh");
    expect(html).toContain('aria-valuenow="75"');
    expect(html).not.toContain('aria-valuenow="100"');
    account.quotaWindowUsage = null;
    expect(ownerAccountValue(account, now).remaining).toBe("—");
  });

  test("quota percentages distinguish observed zero from absent or invalid values", () => {
    for (const unknown of [null, NaN, Infinity, -1, 10001, 1.5]) expect(ownerQuotaPercent(unknown)).toBeNull();
    expect(ownerQuotaPercent(0)).toBe(0);
    expect(ownerQuotaPercent(10000)).toBe(100);
    expect(ownerQuotaPercent(7550)).toBe(76);
    const account = previewOwnerAccount(now, "normal");
    account.quota.primary!.availableBasisPoints = 0;
    account.quota.secondary!.availableBasisPoints = null;
    const html = body(account);
    expect(html).toContain('data-level="critical"');
    expect(html).toContain('aria-valuenow="0"');
    expect(html).toContain('data-level="unknown"');
    expect(html).toContain('aria-valuetext="Unknown"');
    expect(body(previewOwnerAccount(now, "low"))).toContain('data-level="low"');
  });

  test("supplemental windows keep order and exclude Desktop Fast quotas", () => {
    const account = previewOwnerAccount(now, "normal");
    account.quota.supplemental = [
      { id: "fast", label: "Fast", window: account.quota.primary! },
      { id: "ultrafast", label: "UltraFast", window: account.quota.primary! },
      { id: "other", label: "Images", window: account.quota.primary! },
    ];
    expect(ownerQuotaWindows(account).map(item => item.id)).toEqual(["primary", "secondary", "other"]);
    expect(ownerQuotaWindows(account).map(item => item.label)).toEqual(["5 hours", "Week", "Images · 5 hours"]);
    account.quotaStatus = "requires_reauth";
    expect(ownerQuotaWindows(account)).toEqual([]);
    expect(body(account)).toContain("Sign in again");
    expect(body(account)).not.toContain('role="meter"');
  });

  test("empty telemetry keeps failure/refresh states explicit and does not fabricate dates", () => {
    const account = previewOwnerAccount(now, "normal");
    account.quota = {};
    account.subscriptionUntilMs = null;
    for (const status of ["failed", "refreshing", "pending", "not_available"] as const) {
      account.quotaStatus = status;
      expect(body(account)).toContain(`data-state="${status}"`);
    }
    expect(body(account)).not.toContain('class="owner-account-token"');
    expect(body(account)).not.toContain('class="owner-account-subscription"');
    account.tokenExpiresAtMs = null;
    expect(body(account)).toContain('class="owner-account-token"');
    account.tokenExpiresAtMs = now - 1;
    expect(body(account)).toContain("Expired");
    account.quotaStatus = "requires_reauth";
    expect(body(account)).not.toContain('class="owner-account-token"');
    expect(ownerRemainingTime(NaN, now)).toBe("—");
  });

  test("provider credits, expiry and economics are distinct sections in Desktop order", () => {
    const account = previewOwnerAccount(now, "normal");
    account.quota.availableCreditsMicroUnits = 1500000;
    account.tokenExpiresAtMs = now + 3600000;
    const html = body(account);
    const sections = ["owner-account-quota", "owner-provider-credits", "owner-account-token", "owner-account-subscription", "owner-account-values"].map(name => html.indexOf(`class="${name}"`));
    expect(sections.every(index => index >= 0)).toBe(true);
    expect(sections).toEqual([...sections].sort((a, b) => a - b));
    expect(html).toContain("1.5");
    account.quota.providerCreditsUnlimited = true;
    expect(body(account)).toContain("∞");
    expect(renderToStaticMarkup(<LocaleProvider><OwnerAccountBody account={account} nowMs={now} valuesVisible={false} /></LocaleProvider>)).not.toContain('class="owner-account-values"');
  });
});
