import { useEffect, useRef, useState } from "react";
import { ControlError } from "../../api/control";
import { downloadUsageReport, type UsageFilter } from "../../api/usageReport";
import { Download } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { reportErrorLabel } from "./ReportTools";

export function ReportExport({ filter }: { filter: UsageFilter }) {
  const { t } = useLocale();
  const request = useRef<AbortController>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => () => request.current?.abort(), []);

  async function save() {
    if (request.current) return;
    const controller = new AbortController();
    request.current = controller;
    setBusy(true);
    setError(undefined);
    try {
      const data = await downloadUsageReport(filter, AbortSignal.any([controller.signal, AbortSignal.timeout(12000)]));
      controller.signal.throwIfAborted();
      const href = URL.createObjectURL(data);
      try {
        const a = document.createElement("a");
        a.href = href;
        a.download = "relay-usage.csv";
        document.body.append(a);
        try { a.click(); } finally { a.remove(); }
      } finally {
        setTimeout(() => URL.revokeObjectURL(href), 1000);
      }
    } catch (e) {
      if (controller.signal.aborted) return;
      if (e instanceof ControlError && e.status === 401) location.replace("/login");
      else setError(e instanceof ControlError ? e.code ?? "unavailable" : "unavailable");
    } finally {
      request.current = null;
      if (!controller.signal.aborted) setBusy(false);
    }
  }

  return <div className="report-export">
    <button type="button" className="button" disabled={busy} onClick={() => void save()}>
      <Download aria-hidden="true" />{t(busy ? "report.preparingCsv" : "report.downloadCsv")}
    </button>
    {error && <small role="alert">{t(reportErrorLabel(error, true))}</small>}
  </div>;
}
