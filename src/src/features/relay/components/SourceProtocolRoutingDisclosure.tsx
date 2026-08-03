import { Route } from "lucide-react";
import { useId } from "react";
import { useTranslation } from "react-i18next";
import type { SourceProtocolBinding, SourceSummary, SourceWireApi } from "../api/types";
import {
  effectiveSourceProtocolBindings,
  sourceWireApis,
} from "../sourceProtocolBindings";
import { SourceProtocolBindingsEditor } from "./SourceProtocolBindingsEditor";

export function SourceProtocolBindingsSummary({
  source,
}: {
  source: Pick<SourceSummary, "wireApi" | "protocolBindings" | "models">;
}) {
  const { t } = useTranslation();
  const hasRoute = effectiveSourceProtocolBindings(source).some(
    (binding) => binding.modelIds.length > 0,
  );
  return (
    <span className="source-protocol-summary">
      {t(hasRoute ? "sources.routingSummary" : "sources.routingPending")}
    </span>
  );
}

/**
 * The route table is Relay configuration, not a normal setup choice. Keep the
 * declared capabilities visible only when an operator needs to verify an
 * unusual source, while the default flow remains "connect source, pick model".
 */
export function SourceProtocolRoutingDisclosure({
  models,
  value,
  onChange,
  wireApis = sourceWireApis,
}: {
  models: string[];
  value: SourceProtocolBinding[];
  onChange: (value: SourceProtocolBinding[]) => void;
  wireApis?: readonly SourceWireApi[];
}) {
  const { t } = useTranslation();
  const titleId = useId();
  return (
    <section className="source-routing-disclosure" aria-labelledby={titleId}>
      <div className="source-routing-overview">
        <span className="source-routing-icon" aria-hidden="true"><Route /></span>
        <span>
          <strong id={titleId}>{t("sources.routingTitle")}</strong>
          <small>{t("sources.modelRoutingHint")}</small>
        </span>
      </div>
      <details className="source-routing-details">
        <summary>
          <span>{t("sources.routingAdvanced")}</span>
          <small>{t("sources.routingAdvancedHint")}</small>
        </summary>
        <SourceProtocolBindingsEditor
          models={models}
          value={value}
          onChange={onChange}
          wireApis={wireApis}
        />
      </details>
    </section>
  );
}
