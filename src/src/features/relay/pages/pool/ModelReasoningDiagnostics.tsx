import { useTranslation } from "react-i18next";
import type { ModelSummary } from "../../api/types";
import { modelReasoningDiagnostics } from "./reasoningDiagnosticsModel";

export function ModelReasoningDiagnostics({ model }: { model: ModelSummary }) {
  const { t } = useTranslation();
  const diagnostic = modelReasoningDiagnostics(model);
  const key = "models.reasoningDiagnostics";
  const unknown = t(`${key}.notReported`);
  const levels = (values: string[] | null) => values === null
    ? unknown : values.length ? values.join(", ") : t(`${key}.noLevels`);
  return <details className="model-reasoning-diagnostics" open={!diagnostic.supportedLevels?.length || undefined}>
    <summary>{t(`${key}.title`)}</summary>
    <p role="status">{t(`${key}.status.${diagnostic.status}`)}</p>
    <dl>
      <dt>{t(`${key}.source`)}</dt><dd>{diagnostic.source ?? unknown}</dd>
      <dt>{t(`${key}.referenceModel`)}</dt><dd>{diagnostic.referenceModel ?? unknown}</dd>
      <dt>{t(`${key}.support`)}</dt><dd>{diagnostic.support === null ? unknown : t(`${key}.${diagnostic.support ? "declared" : "unsupported"}`)}</dd>
      <dt>{t(`${key}.method`)}</dt><dd>{t(`${key}.methods.${diagnostic.method}`)}</dd>
      <dt>{t(`${key}.referenceLevels`)}</dt><dd>{levels(diagnostic.referenceLevels)}</dd>
      <dt>{t(`${key}.supportedLevels`)}</dt><dd>{levels(diagnostic.supportedLevels)}</dd>
      <dt>{t(`${key}.allowedLevels`)}</dt><dd>{levels(diagnostic.allowedLevels)}</dd>
      <dt>{t(`${key}.effectiveLevels`)}</dt><dd>{levels(diagnostic.effectiveLevels)}</dd>
    </dl>
  </details>;
}
