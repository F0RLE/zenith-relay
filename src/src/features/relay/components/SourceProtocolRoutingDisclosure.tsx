import { useTranslation } from "react-i18next";
import type { SourceProtocolBinding, SourceSummary, SourceWireApi } from "../api/types";
import { effectiveSourceProtocolBindings, sourceWireApis } from "../sourceProtocolBindings";
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

export function SourceProtocolRoutingDisclosure({
  models,
  value,
  onChange,
  wireApis = sourceWireApis,
  showSimplePicker = false,
  autoAssignModels = true,
  exclusiveSimplePicker = false,
}: {
  models: string[];
  value: SourceProtocolBinding[];
  onChange: (value: SourceProtocolBinding[]) => void;
  wireApis?: readonly SourceWireApi[];
  showSimplePicker?: boolean;
  autoAssignModels?: boolean;
  exclusiveSimplePicker?: boolean;
}) {
  return (
    <SourceProtocolBindingsEditor
      models={models}
      value={value}
      onChange={onChange}
      wireApis={wireApis}
      showSimplePicker={showSimplePicker}
      autoAssignModels={autoAssignModels}
      exclusiveSimplePicker={exclusiveSimplePicker}
    />
  );
}
