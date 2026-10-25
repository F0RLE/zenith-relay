import type {
  AccountSummary,
  SourceSummary,
} from "./api/types";

export type PoolMember =
  | (AccountSummary & { kind: "account" })
  | (SourceSummary & { kind: "source" });

export function memberName(member: PoolMember) {
  return member.kind === "source" ? member.name : member.identityHint || member.label;
}

export function toggle(selectedIds: string[], idToToggle: string) {
  return selectedIds.includes(idToToggle)
    ? selectedIds.filter((selectedId) => selectedId !== idToToggle)
    : [...selectedIds, idToToggle];
}

export function compareStableText(left: string, right: string) {
  return left === right ? 0 : left < right ? -1 : 1;
}
