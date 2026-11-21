import { rulesByModel } from "../../api/catalog";
import type { SavedSource } from "../../api/connections";
import { Dialog } from "../../components/Dialog";
import { useLocale } from "../../i18n/LocaleProvider";
import { ModelPriceEditor, SourceMarkupEditor } from "./ModelPriceEditor";
import { SourceModelCheck } from "./SourceModelCheck";
import { useOwnerRules } from "./useOwnerRules";

function ModelList({ models }: { models: string[] }) {
  return <ul className="owner-source-model-list">{models.map(id => <li key={id}><code>{id}</code></li>)}</ul>;
}

function unverifiedModels(source: SavedSource) {
  const verified = new Set(source.verified_models.map(model => model.toLowerCase()));
  return source.declared_models?.filter(model => !verified.has(model.toLowerCase())) ?? [];
}

function UnverifiedModels({ models }: { models: string[] }) {
  const { t } = useLocale();
  return models.length > 0 && <section aria-label={t("sourceIntake.unverified")}>
    <small>{t("sourceIntake.unverified")}</small><ModelList models={models} />
  </section>;
}

type Props = { source: SavedSource; csrf: string; onClose: () => void; webChecks: boolean; canCheck: boolean; refresh: () => void };

function CheckPanel({ source, csrf, webChecks, canCheck, refresh }: Props) {
  return webChecks && source.endpoint && <SourceModelCheck source={source} csrf={csrf}
    allowed={canCheck && source.serving?.state === "enabled"} onVerified={refresh} />;
}

function ConnectionPrices(props: Props) {
  const { source, csrf, onClose } = props;
  const { t } = useLocale();
  const { rows, busy, blocked, apply, feedback } = useOwnerRules(csrf);
  const admitted = rows?.filter(row => row.source_id === source.id && row.admitted) ?? [];
  const target = admitted[0];
  const models = new Set(admitted.map(row => row.model));
  const rest = source.verified_models.filter(model => !models.has(model));
  const groups = rulesByModel((rows ?? []).filter(row => models.has(row.model)));

  return <Dialog className="owner-source-dialog owner-price-dialog" title={t("source.section.models")} onClose={onClose} dismissible={!busy}>
    <strong className="saved-source-name">{source.label ?? t("connections.apiProvider")}</strong>
    {feedback}
    {target && <>
      <SourceMarkupEditor key={`${target.source_id}:${target.source_markup.revision}`} source={target} busy={blocked} apply={apply} />
      <small className="catalog-unit">{t("modelRules.units")}</small>
      <ul className="catalog-list">{Array.from(groups, ([model, rules]) =>
        <ModelPriceEditor key={model} sources={rules} sourceId={source.id} busy={blocked} apply={apply} />)}
      </ul>
    </>}
    {rest.length > 0 && <ModelList models={rest} />}
    {rows && !target && !rest.length && <p>{t("connections.noVerifiedModels")}</p>}
    <UnverifiedModels models={unverifiedModels(source)} />
    <CheckPanel {...props} />
  </Dialog>;
}

export function ConnectionModels(props: Props) {
  const { t } = useLocale();
  const { source, onClose } = props;
  if (source.serving?.in_pool) return <ConnectionPrices {...props} />;
  const unverified = unverifiedModels(source);
  return <Dialog className="owner-source-dialog" title={t(unverified.length || props.webChecks && source.endpoint
    ? "source.section.models" : "connections.verifiedModels")} onClose={onClose}>
    <strong className="saved-source-name">{source.label ?? t("connections.apiProvider")}</strong>
    {source.verified_models.length ? <ModelList models={source.verified_models} /> : <p>{t("connections.noVerifiedModels")}</p>}
    <UnverifiedModels models={unverified} />
    <CheckPanel {...props} />
  </Dialog>;
}
