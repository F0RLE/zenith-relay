import { control, record, uuidText } from "./control";
import { parseServing, type Serving } from "./connections";
import { confirmSupplyAction } from "./ownerSession";
import { canonicalRetailRate } from "../features/models/retailRate";

export const components = ["input", "cached", "output"] as const;
export type Prices = Record<typeof components[number], string>;
export type MarkupRule = { mode: "api_default" } | { mode: "model_markup"; percent: string };
export type SaleMode = "fixed" | MarkupRule["mode"];
interface SourceMarkup { revision: string; percent: string | null }
interface ReviewedRule extends SourceMarkup { mode: SaleMode; stale: boolean; below_cost: boolean }
export interface RuleFields {
  rates?: Prices;
  purchase_rates?: Prices;
  source_markup?: { expected_revision: string; percent: string | null };
  sale_rule?: MarkupRule;
}
export type RuleAction = "admit" | "save" | "review" | "publish" | "suspend";
export const confirmsRuleAction = (action: RuleAction) => action !== "save" && action !== "review";
export interface Rule {
  id: string; model: string; source_id: string; source: string; proxy: boolean; revision: string;
  admitted: boolean; enabled: boolean; ready: boolean; supply_ready: boolean;
  purchase: Prices | null; draft: Prices | null; effective: Prices | null;
  source_markup: SourceMarkup; sale_rule: ReviewedRule;
  serving: Serving | null;
}
interface CatalogIdentity { model: string; publisher: string | null }
export interface Listing extends CatalogIdentity { rates: Prices; revision: string; protocol: "responses" }
export interface ModelDisplayOrder { revision: string; items: CatalogIdentity[] }
const invalid = (): never => { throw new Error("Invalid catalog response"); };
const id = (v: unknown): v is string => typeof v === "string" && /^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$/.test(v) && v.trim() === v;
const revision = (v: unknown): v is string => typeof v === "string" && /^(?:0|[1-9]\d{0,18})$/.test(v) && v.trim() === v;
const orderRevision = (v: unknown): v is string => typeof v === "string" && v.length === 64 && /^[0-9a-f]+$/.test(v);
function identity(v: Record<string, unknown>): CatalogIdentity {
  const { model, publisher } = v;
  if (!id(model) || !(publisher === null || typeof publisher === "string" && publisher !== "other"
    && publisher.trim() === publisher && /^[a-z0-9][a-z0-9_-]{0,63}$/.test(publisher))) return invalid();
  return { model, publisher };
}
export function textRate(v: unknown) {
  const s = canonicalRetailRate(v);
  return s && s.indexOf(".") <= 12 ? s : null;
}
function prices(v: unknown): Prices {
  if (!record(v)) return invalid();
  return { input: textRate(v.input) ?? invalid(), cached: textRate(v.cached) ?? invalid(), output: textRate(v.output) ?? invalid() };
}
function purchasePrices(v: unknown) {
  if (v === null) return null;
  if (!record(v) || Object.keys(v).length !== components.length) return invalid();
  const parsed = prices(v);
  return components.every(k => v[k] === parsed[k]) ? parsed : invalid();
}
const validMarkup = (v: Record<string, unknown>): v is Record<string, unknown> & SourceMarkup =>
  revision(v.revision) && (v.percent === null || (typeof v.percent === "string" && textRate(v.percent) === v.percent));
function items<T>(raw: unknown, parse: (v: Record<string, unknown>) => T, key: (t: T) => string) {
  if (!Array.isArray(raw) || raw.length > 128) return invalid();
  const rows = raw.map(v => record(v) ? parse(v) : invalid());
  if (new Set(rows.map(key)).size !== rows.length) return invalid();
  return rows;
}
export function parseListings(raw: unknown): Listing[] {
  return items<Listing>(raw, v => {
    if (v.protocol !== "responses" || !revision(v.revision) || v.revision === "0") return invalid();
    return { ...identity(v), protocol: v.protocol, revision: v.revision, rates: prices(v.rates) };
  }, row => row.model);
}

export function parseModelDisplayOrder(raw: Record<string, unknown>): ModelDisplayOrder {
  if (!orderRevision(raw.revision)) return invalid();
  const rows = items(raw.items, identity, row => row.model);
  const groups = new Set<string | null>();
  let previous: string | null | undefined;
  for (const row of rows) {
    if (row.publisher === previous) continue;
    if (groups.has(row.publisher)) return invalid();
    groups.add(row.publisher);
    previous = row.publisher;
  }
  return { revision: raw.revision, items: rows };
}
export function parseRules(raw: unknown): Rule[] {
  return items<Rule>(raw, v => {
    const markup = v.source_markup, sale = v.sale_rule;
    if (typeof v.id !== "string" || !uuidText.test(v.id) || !id(v.model) || !revision(v.revision)
      || typeof v.source_id !== "string" || v.source_id.length !== 36 || !uuidText.test(v.source_id)
      || typeof v.source !== "string" || !v.source.trim() || v.source.length > 96 || /\p{Cc}/u.test(v.source)
      || typeof v.proxy !== "boolean" || typeof v.admitted !== "boolean" || typeof v.ready !== "boolean"
      || typeof v.enabled !== "boolean" || typeof v.supply_ready !== "boolean"
      || (v.enabled && !v.admitted && (v.ready || v.purchase !== null))
      || (v.supply_ready && v.enabled) !== (v.effective !== null)
      || !record(markup) || !validMarkup(markup) || (markup.revision === "0" && markup.percent !== null)
      || !record(sale) || !validMarkup(sale) || (sale.mode !== "fixed" && sale.mode !== "api_default" && sale.mode !== "model_markup")
      || typeof sale.stale !== "boolean" || typeof sale.below_cost !== "boolean"
      || (sale.mode === "model_markup") !== (sale.percent !== null)
      || (sale.mode === "fixed" && sale.stale)
      || (sale.mode !== "fixed" && (v.draft === null || sale.revision === "0"))) return invalid();
    const serving = parseServing(v.serving);
    if (v.admitted && serving && !serving.in_pool) return invalid();
    return { id: v.id, model: v.model, revision: v.revision, source_id: v.source_id, source: v.source, proxy: v.proxy, admitted: v.admitted, serving,
      ready: v.ready, supply_ready: v.supply_ready, enabled: v.enabled, purchase: purchasePrices(v.purchase),
      source_markup: { revision: markup.revision, percent: markup.percent },
      sale_rule: { revision: sale.revision, percent: sale.percent, mode: sale.mode, stale: sale.stale, below_cost: sale.below_cost },
      draft: v.draft === null ? null : prices(v.draft), effective: v.effective === null ? null : prices(v.effective) };
  }, row => row.id);
}

export function rulesByModel(rows: Rule[]): Map<string, Rule[]> {
  const models = new Map<string, Rule[]>();
  for (const row of rows) {
    const group = models.get(row.model);
    if (group) group.push(row);
    else models.set(row.model, [row]);
  }
  return models;
}

const rulesPath = "admin/pools/model-rules";
const orderPath = "admin/pools/model-display-order";
export const readRules = async () => parseRules((await control(rulesPath)).items);
export const readListings = async () => parseListings((await control("models")).items);
export const readModelDisplayOrder = async () => parseModelDisplayOrder(await control(orderPath));
export async function saveModelDisplayOrder(csrf: string, expected_revision: string, model_display_order: string[]) {
  return parseModelDisplayOrder(await control(orderPath, {
    method: "PUT", headers: { "Content-Type": "application/json", "X-Relay-CSRF": csrf },
    body: JSON.stringify({ expected_revision, model_display_order }),
  }));
}

export async function changeRule(csrf: string, item: Rule, action: RuleAction, op: string, fields?: RuleFields) {
  const path = `${rulesPath}/${item.id}`;
  const command = { operation_id: op, expected_revision: item.revision, action, ...fields };
  if (confirmsRuleAction(action)) return confirmSupplyAction(path, csrf, command);
  const result = await control(path, {
    method: "POST", headers: { "Content-Type": "application/json", "X-Relay-CSRF": csrf },
    body: JSON.stringify(command),
  });
  if (result.applied !== true || result.operation_id !== op) return invalid();
}
