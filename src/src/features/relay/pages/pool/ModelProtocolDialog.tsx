import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ModelSummary, ProtocolFeature, SourceWireApi } from "../../api/types";
import { Button, Dialog, Tabs } from "../../components/Ui";
import { sourceWireApis } from "../../sourceProtocolBindings";
import { formatReasoningEffort } from "../../poolFormatting";

const features: ProtocolFeature[] = ["text", "streaming", "images", "function_tools", "tool_choice", "structured_output", "reasoning"];

export function ModelProtocolDialog({ model, onClose }: { model: ModelSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const [protocol, setProtocol] = useState<SourceWireApi>("responses");
  const routes = model.protocolRoutes?.filter((route) => route.clientWireApi === protocol) ?? [];
  return <Dialog wide className="model-protocol-dialog" title={t("models.protocolTitle")} onClose={onClose}
    footer={<Button variant="primary" onClick={onClose}>{t("common.close")}</Button>}>
    <code className="model-protocol-name">{model.id}</code>
    <Tabs value={protocol} label={t("models.clientProtocol")} onChange={(value) => setProtocol(value as SourceWireApi)}
      items={sourceWireApis.map((id) => ({ id, label: t(`sources.protocolCards.${id}.title`) }))} />
    <div role="tabpanel" aria-label={t(`sources.protocolCards.${protocol}.title`)} className="model-protocol-routes">
      {!model.enabled ? <p className="model-protocol-empty">{t("models.disabled")}</p> : null}
      {!routes.length ? <p className="model-protocol-empty">{t(model.protocolRoutes ? "models.noProtocolRoute" : "models.protocolUnknown")}</p> : null}
      {routes.map((route, index) => <section key={index} className="model-protocol-route" data-upstream={route.upstreamWireApi}>
        <h3>{t(`sources.protocolCards.${route.upstreamWireApi}.title`)}<span>{t(route.upstreamWireApi === protocol ? "models.nativeProtocol" : "models.convertedProtocol")}</span></h3>
        <dl>{features.map((feature) => {
          const status = route.features[feature] ?? "unknown";
          return <div key={feature} data-feature={feature} data-status={status}><dt>{t(`models.protocolFeatures.${feature}`)}</dt><dd>{t(`models.capabilityStatuses.${status}`)}</dd></div>;
        })}
          {route.reasoningEfforts.length ? <div className="model-protocol-efforts"><dt>{t("models.reasoningTitle")}</dt><dd>{route.reasoningEfforts.map((effort) => t(`usage.reasoningEfforts.${effort}`, { defaultValue: formatReasoningEffort(effort) })).join(", ")}</dd></div> : null}
        </dl>
      </section>)}
    </div>
  </Dialog>;
}
