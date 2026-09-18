import { FlaskConical } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import type { SourceProbeResult, SourceSummary, SourceWireApi } from "../api/types";
import { sourceModelsForWireApi, sourceWireApis, upstreamWireApi, effectiveSourceProtocolBindings } from "../sourceProtocolBindings";
import { useRelayState } from "../state/RelayStateProvider";
import { Button, OptionMenu } from "./Ui";

export function SourceProtocolStatus({ source: initial, dirty = false, onConfirmed }: { source: SourceSummary; dirty?: boolean; onConfirmed?: (() => Promise<void>) | undefined }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const source = runtime?.sources.find((value) => value.id === initial.id) ?? initial;
  const [model, setModel] = useState(source.models[0] ?? "");
  const [selectedProtocol, setSelectedProtocol] = useState<SourceWireApi | null>(null);
  const availableModel = source.models.includes(model) ? model : source.models[0] ?? "";
  const detectedRoute = effectiveSourceProtocolBindings(source).find((route) => route.modelIds.includes(availableModel));
  const wireApi = selectedProtocol ?? (detectedRoute ? upstreamWireApi(detectedRoute) : source.protocolConfig?.endpointHint ?? source.wireApi);
  const [result, setResult] = useState<SourceProbeResult | null>(null);
  const supported = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("source_protocols_v1"));
  const probe = () => perform(`source-probe-${source.id}`, async () => {
    setResult(null);
    const input = { modelId: availableModel, wireApi, expectedRevision: source.protocolConfig?.revision ?? 0 };
    const result = await (mode === "remote" ? relayCommands.probeRemoteSource(source.id, input) : relayCommands.probeSource(source.id, input));
    setResult(result);
    if (result.capability.status === "confirmed") await onConfirmed?.();
  });
  return <section className="source-protocol-status">
    <div className="source-protocol-availability">{sourceWireApis.map((protocol) => {
      const count = sourceModelsForWireApi(source, protocol).length;
      return <div key={protocol} data-available={count > 0}><span>{t(`sources.protocolCards.${protocol}.title`)}</span>
        <strong>{count ? t("sources.protocolModelCount", { count }) : t("sources.routingPending")}</strong></div>;
    })}</div>
    {supported ? <div className="source-probe-controls">
      <label className="relay-field"><span>{t("sources.modelColumn")}</span><OptionMenu className="field-option-menu" label={t("sources.modelColumn")} value={availableModel} onChange={(value) => { setModel(value); setSelectedProtocol(null); setResult(null); }} options={source.models.map((id) => ({ value: id, label: id }))} /></label>
      <label className="relay-field"><span>{t("sources.probeFormat")}</span><OptionMenu className="field-option-menu" label={t("sources.probeFormat")} value={wireApi} onChange={(value) => { setSelectedProtocol(value as SourceWireApi); setResult(null); }} options={sourceWireApis.map((protocol) => ({ value: protocol, label: t(`sources.protocolCards.${protocol}.title`) }))} /></label>
      <Button icon={<FlaskConical aria-hidden />} busy={busy === `source-probe-${source.id}`} disabled={!availableModel || dirty || Boolean(busy)} title={dirty ? t("sources.probeSaveFirst") : t("sources.probeCost")} onClick={() => void probe()}>{t("sources.probeGeneration")}</Button>
    </div> : null}
    {result && !dirty && result.revision === (source.protocolConfig?.revision ?? 0) && result.capability.modelId === availableModel && result.capability.upstreamWireApi === wireApi ? <div role="status" className="source-probe-result" data-status={result.capability.status}>
      <strong>{t(`sources.probeStatuses.${result.capability.status}`)}</strong>{result.httpStatus ? <span>HTTP {result.httpStatus}</span> : null}
      {result.errorCode ? <code>{result.errorCode}</code> : null}
    </div> : null}
  </section>;
}
