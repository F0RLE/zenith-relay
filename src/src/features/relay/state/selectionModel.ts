/** Toggle one identity without changing the order of the other selections. */
export function toggleSelectedId(selectedIds: readonly string[], targetId: string): string[] {
  return selectedIds.includes(targetId)
    ? selectedIds.filter((selectedId) => selectedId !== targetId)
    : [...selectedIds, targetId];
}

/** Change only the visible group, preserving selections hidden by a filter. */
export function setVisibleIdsSelected(
  selectedIds: readonly string[],
  visibleIds: readonly string[],
  selected: boolean,
): string[] {
  if (selected) return [...new Set([...selectedIds, ...visibleIds])];
  const visible = new Set(visibleIds);
  return selectedIds.filter((selectedId) => !visible.has(selectedId));
}
