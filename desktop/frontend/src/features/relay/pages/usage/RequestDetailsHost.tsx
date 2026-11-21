import { forwardRef, memo, useCallback, useEffect, useImperativeHandle, useState } from "react";
import { RequestDetails } from "./RequestDetails";
import type { UsageRow } from "./usageData";

export type RequestDetailsHandle = {
  open: (row: UsageRow) => void;
  close: () => void;
};

/** Opening/closing a report must not reconcile the underlying Usage workspace. */
export const RequestDetailsHost = memo(forwardRef<RequestDetailsHandle, { rows: UsageRow[]; local: boolean }>(
  function RequestDetailsHost({ rows, local }, ref) {
    const [selected, setSelected] = useState<UsageRow | null>(null);
    const close = useCallback(() => setSelected(null), []);
    useImperativeHandle(ref, () => ({ open: setSelected, close }), [close]);

    useEffect(() => {
      if (!selected) return;
      const current = rows.find((row) => selected.requestId
        ? row.requestId === selected.requestId && row.attempt === selected.attempt
        : row.id === selected.id);
      if (current && current !== selected) setSelected(current);
    }, [rows, selected]);

    return selected ? <RequestDetails
      key={`${selected.requestId ?? selected.id}:${selected.attempt}`}
      row={selected}
      local={local}
      onClose={close}
    /> : null;
  },
));
