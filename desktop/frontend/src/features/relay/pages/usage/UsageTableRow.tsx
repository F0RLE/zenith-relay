import { memo, type ReactNode } from "react";
import type { UsageRow } from "./usageData";

export type UsageColumns<Key extends string> = Record<Key, { label: string; cell: (row: UsageRow) => ReactNode }>;

type RowProps<Key extends string> = {
  row: UsageRow;
  columns: UsageColumns<Key>;
  order: Key[];
};

function TableRow<Key extends string>({ row, columns, order }: RowProps<Key>) {
  return <tr>{order.map((columnId) => <td key={columnId} data-column={columnId}
    data-label={columns[columnId].label}
    data-relay-tooltip={columnId === "model" ? row.model ?? undefined : columnId === "connection" ? row.connection : undefined}
  >{columns[columnId].cell(row)}</td>)}</tr>;
}

export const UsageTableRow = memo(TableRow, (previous, next) => previous.columns === next.columns
  && previous.order === next.order && sameUsageValue(previous.row, next.row)) as typeof TableRow;

// IPC returns fresh JSON objects. Compare the bounded visible row, including
// details, so unchanged sends skip formatting/rendering while corrections
// still replace the click handler's data. No second history cache is retained.
function sameUsageValue(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (!left || !right || typeof left !== "object" || typeof right !== "object"
    || Array.isArray(left) !== Array.isArray(right)) return false;
  const keys = Object.keys(left);
  const before = left as Record<string, unknown>;
  const after = right as Record<string, unknown>;
  return keys.length === Object.keys(right).length
    && keys.every((key) => Object.hasOwn(after, key) && sameUsageValue(before[key], after[key]));
}
