import type { AccountSummary, SourceSummary } from "../../api/types";
import { accountSurfaceTone, operationalStatusTone } from "../../accountStatus";

export type MemberPickerTone = "ready" | "warning" | "error" | "info" | "disabled";
export type MemberPickerHealth = "ready" | "cooldown" | "error" | "disabled" | "info";

/** Same order as the pool cards: working, cooldown, unavailable, then the rest. */
export const MEMBER_PICKER_HEALTH_ORDER = ["ready", "cooldown", "error", "disabled", "info"] as const;

export function memberPickerHealth(tone: MemberPickerTone): MemberPickerHealth {
  return tone === "warning" ? "cooldown" : tone;
}

export function accountPickerTone(account: AccountSummary, onServer: boolean): MemberPickerTone {
  return accountSurfaceTone(account, onServer);
}

export function sourcePickerTone(source: Pick<SourceSummary, "lastErrorCode" | "operationalStatus">): MemberPickerTone {
  if (source.lastErrorCode?.trim()) return "error";
  return operationalStatusTone(source.operationalStatus);
}

export function compareMemberPickerHealth(left: MemberPickerHealth, right: MemberPickerHealth) {
  return MEMBER_PICKER_HEALTH_ORDER.indexOf(left) - MEMBER_PICKER_HEALTH_ORDER.indexOf(right);
}
