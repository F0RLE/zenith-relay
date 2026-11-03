//! Shared presentation policy. Provider/family/class rules live in the public
//! JSON catalog; the code only parses identities and compares numeric versions.

use chrono::{Datelike, NaiveDate};
use regex::{Regex, RegexSet};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, collections::BTreeSet, sync::LazyLock};

pub const MODEL_ORDER_JSON: &str = include_str!("../../../../catalog/model-order.json");
const MODEL_ORDER_SCHEMA: &str = include_str!("../../../../catalog/model-order.schema.json");

static POLICY: LazyLock<OrderPolicy> =
    LazyLock::new(|| OrderPolicy::parse().expect("the bundled model order catalog must be valid"));
static VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9]+(?:\.[0-9]+)*").expect("valid version pattern"));
static SNAPSHOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:[-@](?P<compact>[0-9]{8})|-(?P<iso>[0-9]{4}-[0-9]{2}-[0-9]{2}))$")
        .expect("valid snapshot pattern")
});

/// Display classification only. It neither rewrites an upstream ID nor grants
/// capabilities, account entitlement, or a route.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDisplayIdentity {
    pub provider: Option<String>,
    pub family: String,
    pub class: String,
    pub version: Vec<u64>,
    pub variant: String,
    pub snapshot: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OrderDocument {
    #[serde(rename = "$schema")]
    schema: String,
    schema_version: u32,
    revision: String,
    ignored_suffixes: Vec<String>,
    providers: Vec<ProviderRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderRule {
    id: String,
    aliases: Vec<String>,
    patterns: Vec<String>,
    families: Vec<FamilyRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FamilyRule {
    id: String,
    patterns: Vec<String>,
    classes: Vec<ClassRule>,
    variants: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClassRule {
    id: String,
    aliases: Vec<String>,
}

struct OrderPolicy {
    document: OrderDocument,
    provider_patterns: Vec<RegexSet>,
    family_patterns: Vec<Vec<Vec<Regex>>>,
}

impl OrderPolicy {
    fn parse() -> Result<Self, String> {
        let schema = serde_json::from_str(MODEL_ORDER_SCHEMA).map_err(|error| error.to_string())?;
        let document = serde_json::from_str(MODEL_ORDER_JSON).map_err(|error| error.to_string())?;
        jsonschema::options()
            .offline()
            .build(&schema)
            .map_err(|error| error.to_string())?
            .validate(&document)
            .map_err(|error| error.to_string())?;
        let document: OrderDocument =
            serde_json::from_value(document).map_err(|error| error.to_string())?;
        if document.schema != "model-order.schema.json" || document.schema_version != 1 {
            return Err("unsupported model order schema".into());
        }
        let mut provider_ids = BTreeSet::new();
        let mut provider_patterns = Vec::new();
        let mut family_patterns = Vec::new();
        for provider in &document.providers {
            for id in std::iter::once(&provider.id).chain(&provider.aliases) {
                if !provider_ids.insert(id) {
                    return Err(format!("duplicate provider identity: {id}"));
                }
            }
            let compiled = compile_patterns(&provider.patterns)?;
            provider_patterns.push(
                RegexSet::new(compiled.iter().map(Regex::as_str))
                    .map_err(|error| error.to_string())?,
            );
            let mut family_ids = BTreeSet::new();
            let mut compiled_families = Vec::new();
            for family in &provider.families {
                if !family_ids.insert(&family.id) {
                    return Err(format!("duplicate family: {}", family.id));
                }
                let mut class_ids = BTreeSet::new();
                let mut class_aliases = BTreeSet::new();
                for class in &family.classes {
                    if !class_ids.insert(&class.id)
                        || class
                            .aliases
                            .iter()
                            .any(|alias| !class_aliases.insert(alias))
                    {
                        return Err(format!("duplicate class identity: {}", family.id));
                    }
                }
                if family
                    .classes
                    .iter()
                    .filter(|class| class.aliases.is_empty())
                    .count()
                    > 1
                {
                    return Err(format!("multiple default classes: {}", family.id));
                }
                compiled_families.push(compile_patterns(&family.patterns)?);
            }
            family_patterns.push(compiled_families);
        }
        Ok(Self {
            document,
            provider_patterns,
            family_patterns,
        })
    }

    fn provider(&self, provider: &str) -> Option<(usize, &ProviderRule)> {
        self.document
            .providers
            .iter()
            .enumerate()
            .find(|(_, rule)| {
                rule.id == provider || rule.aliases.iter().any(|alias| alias == provider)
            })
    }

    fn model_name(&self, model: &str) -> (String, Option<u32>, String) {
        let normalized = normalize(model_leaf(model));
        // Hosting modifiers such as `:batch` or `:thinking` describe a
        // variant; they cannot turn Flash Lite into Flash or hide a snapshot.
        let (name, modifiers) = normalized.split_once(':').unwrap_or((&normalized, ""));
        let mut name = name.to_string();
        let modifiers = modifiers.replace(':', "-");
        for suffix in &self.document.ignored_suffixes {
            if let Some(stripped) = name.strip_suffix(suffix) {
                name = stripped.to_string();
                break;
            }
        }
        let Some(snapshot) = SNAPSHOT.captures(&name) else {
            return (name, None, modifiers);
        };
        let date = snapshot
            .name("iso")
            .map(|date| date.as_str().to_string())
            .or_else(|| {
                let date = snapshot.name("compact")?.as_str();
                Some(format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..]))
            })
            .and_then(|date| date_key(&date));
        match (snapshot.get(0), date) {
            (Some(matched), Some(date)) => {
                (name[..matched.start()].to_string(), Some(date), modifiers)
            }
            _ => (name, None, modifiers),
        }
    }
}

fn compile_patterns(patterns: &[String]) -> Result<Vec<Regex>, String> {
    patterns
        .iter()
        .map(|pattern| {
            if !pattern.starts_with('^') || !pattern.ends_with('$') {
                return Err(format!("unanchored model order pattern: {pattern}"));
            }
            Regex::new(pattern).map_err(|error| error.to_string())
        })
        .collect()
}

pub(super) fn canonical_namespace(model_id: &str) -> Option<String> {
    let (namespace, model) = model_id.trim().split_once('/')?;
    if model.is_empty() {
        return None;
    }
    provider_key(namespace)
}

pub fn model_order_revision() -> &'static str {
    &POLICY.document.revision
}

/// Used by the exporter to report malformed policy as a normal error before
/// constructing a reference catalog.
pub fn validate_model_order_catalog() -> Result<(), String> {
    OrderPolicy::parse().map(|_| ())
}

/// Only the bundled presentation rules infer a missing manufacturer. Trusted
/// reference identity and ambiguous-leaf handling remain with the catalog.
pub(super) fn inferred_provider(model_id: &str) -> Option<String> {
    let (name, _, _) = POLICY.model_name(model_id);
    POLICY
        .document
        .providers
        .iter()
        .enumerate()
        .find_map(|(index, rule)| {
            POLICY.provider_patterns[index]
                .is_match(&name)
                .then(|| rule.id.clone())
        })
}

pub(super) fn provider_key(provider: &str) -> Option<String> {
    let provider = normalize(provider);
    let provider = POLICY
        .provider(&provider)
        .map_or(provider.clone(), |(_, rule)| rule.id.clone());
    (!provider.is_empty()).then_some(provider)
}

pub(super) struct ModelOrderKey {
    pub(super) identity: ModelDisplayIdentity,
    provider_rank: usize,
    family_rank: usize,
    class_rank: usize,
    variant_rank: usize,
    parameter_count: Option<u64>,
    id: String,
}

impl ModelOrderKey {
    pub(super) fn new(
        model: &str,
        provider: Option<String>,
        family_hint: Option<&str>,
        reference_name: Option<&str>,
        release_date: Option<&str>,
    ) -> Self {
        let (name, snapshot, modifiers) = POLICY.model_name(model);
        let provider_rule = provider
            .as_deref()
            .and_then(|provider| POLICY.provider(provider));
        if let Some((provider_rank, rule)) = provider_rule {
            for (family_rank, family) in rule.families.iter().enumerate() {
                for pattern in &POLICY.family_patterns[provider_rank][family_rank] {
                    if let Some(captures) = pattern.captures(&name) {
                        let Ok(captured_snapshot) = captured_snapshot(&captures, release_date)
                        else {
                            continue;
                        };
                        let snapshot = snapshot.or(captured_snapshot);
                        let (class_rank, class, variant) =
                            classify_variant(family, captured_variant(&captures));
                        let version = captures
                            .name("version")
                            .map(|version| version_numbers(version.as_str()))
                            .or_else(|| {
                                version_from_reference_name(
                                    family,
                                    &POLICY.family_patterns[provider_rank][family_rank],
                                    &class,
                                    reference_name?,
                                )
                            })
                            .unwrap_or_default();
                        let variant = join_variants(&variant, &modifiers);
                        return Self {
                            provider_rank,
                            family_rank,
                            class_rank,
                            variant_rank: variant_rank(&family.variants, &variant),
                            parameter_count: captures
                                .name("parameters")
                                .and_then(|size| parameter_count(size.as_str())),
                            identity: ModelDisplayIdentity {
                                provider,
                                family: family.id.clone(),
                                class,
                                version,
                                variant,
                                snapshot,
                            },
                            id: name,
                        };
                    }
                }
            }
        }
        // Unlisted families still get numeric version order, with no guessed
        // capability rank. Metadata can group aliases of a new family.
        let version = VERSION.find(&name);
        let prefix = version.map_or(name.as_str(), |version| &name[..version.start()]);
        let family = family_hint
            .map(normalize)
            .filter(|family| !family.is_empty())
            .unwrap_or_else(|| prefix.trim_matches(['-', '_', '.']).to_string());
        let variant = version
            .map_or("", |version| &name[version.end()..])
            .trim_matches('-');
        Self {
            provider_rank: provider_rule.map_or(POLICY.document.providers.len(), |(rank, _)| rank),
            family_rank: provider_rule.map_or(0, |(_, rule)| rule.families.len()),
            class_rank: 0,
            variant_rank: 0,
            parameter_count: None,
            identity: ModelDisplayIdentity {
                provider,
                family,
                class: prefix.trim_matches(['-', '_', '.']).to_string(),
                version: version.map_or(Vec::new(), |version| version_numbers(version.as_str())),
                variant: join_variants(variant, &modifiers),
                snapshot,
            },
            id: name,
        }
    }

    pub(super) fn compare(&self, other: &Self) -> Ordering {
        self.provider_rank
            .cmp(&other.provider_rank)
            .then_with(|| {
                self.identity
                    .provider
                    .is_none()
                    .cmp(&other.identity.provider.is_none())
            })
            .then_with(|| self.identity.provider.cmp(&other.identity.provider))
            .then_with(|| self.family_rank.cmp(&other.family_rank))
            .then_with(|| natural_compare(&self.identity.family, &other.identity.family))
            .then_with(|| self.class_rank.cmp(&other.class_rank))
            .then_with(|| natural_compare(&self.identity.class, &other.identity.class))
            .then_with(|| compare_versions(&other.identity.version, &self.identity.version))
            .then_with(|| self.variant_rank.cmp(&other.variant_rank))
            .then_with(|| compare_optional_descending(self.parameter_count, other.parameter_count))
            .then_with(|| natural_compare(&self.identity.variant, &other.identity.variant))
            .then_with(|| {
                compare_optional_descending(self.identity.snapshot, other.identity.snapshot)
            })
            .then_with(|| natural_compare(&self.id, &other.id))
    }
}

fn captured_variant(captures: &regex::Captures<'_>) -> String {
    ["variant", "suffix"]
        .into_iter()
        .filter_map(|field| captures.name(field))
        .map(|part| part.as_str().trim_matches('-'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// A dated or size-only ID may omit its generation. A reference name can fill
/// that gap only when the same family rule and class recognize it.
fn version_from_reference_name(
    family: &FamilyRule,
    patterns: &[Regex],
    class: &str,
    reference_name: &str,
) -> Option<Vec<u64>> {
    let name = reference_name.split('(').next()?.rsplit(':').next()?;
    let name = name.split_whitespace().collect::<Vec<_>>().join("-");
    let name = normalize(&name);
    patterns.iter().find_map(|pattern| {
        let captures = pattern.captures(&name)?;
        let version = captures.name("version")?;
        let (_, reference_class, _) = classify_variant(family, captured_variant(&captures));
        (reference_class == class).then(|| version_numbers(version.as_str()))
    })
}

/// Short release tags are opt-in family captures, never generic version
/// guesses. A month/day without a reference year retains year zero.
fn captured_snapshot(
    captures: &regex::Captures<'_>,
    release_date: Option<&str>,
) -> Result<Option<u32>, ()> {
    let date = if let Some(snapshot) = captures.name("snapshot") {
        let date = snapshot.as_str();
        match date.len() {
            4 => format!("20{}-{}-01", &date[..2], &date[2..]),
            6 => format!("20{}-{}-{}", &date[..2], &date[2..4], &date[4..]),
            _ => return Err(()),
        }
    } else if let Some(snapshot) = captures.name("snapshot_month") {
        let (month, year) = snapshot.as_str().split_once('-').ok_or(())?;
        format!("{year}-{month}-01")
    } else if let Some(snapshot) = captures.name("snapshot_day") {
        let date = snapshot.as_str().replace('-', "");
        if date.len() != 4 {
            return Err(());
        }
        let year = release_date
            .and_then(date_key)
            .map_or(0, |date| date / 10_000);
        format!("{year:04}-{}-{}", &date[..2], &date[2..])
    } else {
        return Ok(None);
    };
    date_key(&date).map(Some).ok_or(())
}

/// Parameter counts order sizes within one class, generation and variant.
/// They are presentation data, not a capability or quality score.
fn parameter_count(size: &str) -> Option<u64> {
    let (number, unit) = size.split_at(size.len().checked_sub(1)?);
    let unit = match unit {
        "m" => 1_000_000_u64,
        "b" => 1_000_000_000_u64,
        "t" => 1_000_000_000_000_u64,
        _ => return None,
    };
    let (experts, number) = number.split_once('x').unwrap_or(("1", number));
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    let whole = whole.parse::<u64>().ok()?.checked_mul(unit)?;
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u64>()
            .ok()?
            .checked_mul(unit)?
            .checked_div(10_u64.checked_pow(fraction.len().try_into().ok()?)?)?
    };
    whole
        .checked_add(fraction)?
        .checked_mul(experts.parse().ok()?)
}

fn compare_optional_descending<T: Ord>(left: Option<T>, right: Option<T>) -> Ordering {
    left.is_some()
        .cmp(&right.is_some())
        .then_with(|| right.cmp(&left))
}

fn join_variants(variant: &str, modifiers: &str) -> String {
    [variant, modifiers]
        .into_iter()
        .map(|part| part.trim_matches('-'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn classify_variant(family: &FamilyRule, variant: String) -> (usize, String, String) {
    let matched = family
        .classes
        .iter()
        .enumerate()
        .flat_map(|(rank, class)| {
            let variant = variant.as_str();
            class.aliases.iter().filter_map(move |alias| {
                segment_position(variant, alias).map(|position| (rank, class, alias, position))
            })
        })
        .max_by_key(|(_, _, alias, _)| alias.len());
    if let Some((rank, class, alias, position)) = matched {
        let remaining = [&variant[..position], &variant[position + alias.len()..]]
            .into_iter()
            .map(|part| part.trim_matches('-'))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        return (rank, class.id.clone(), remaining);
    }
    if family.classes.is_empty() {
        return (0, "base".into(), variant);
    }
    if (variant.is_empty() || variant_rank(&family.variants, &variant) < family.variants.len())
        && let Some((rank, class)) = family
            .classes
            .iter()
            .enumerate()
            .find(|(_, class)| class.aliases.is_empty())
    {
        return (rank, class.id.clone(), variant);
    }
    (family.classes.len(), variant, String::new())
}

fn segment_position(text: &str, segment: &str) -> Option<usize> {
    if segment.is_empty() {
        return None;
    }
    text.match_indices(segment).find_map(|(position, _)| {
        let end = position + segment.len();
        ((position == 0 || text.as_bytes()[position - 1] == b'-')
            && (end == text.len() || text.as_bytes()[end] == b'-'))
            .then_some(position)
    })
}

fn variant_rank(variants: &[String], variant: &str) -> usize {
    variants
        .iter()
        .enumerate()
        .filter(|(_, known)| {
            (known.is_empty() && variant.is_empty())
                || (!known.is_empty() && segment_position(variant, known).is_some())
        })
        .max_by_key(|(_, known)| known.len())
        .map_or(variants.len(), |(rank, _)| rank)
}

fn version_numbers(version: &str) -> Vec<u64> {
    version
        .split(['.', '-', '_'])
        .map(str::parse)
        .collect::<Result<_, _>>()
        .unwrap_or_default()
}

fn compare_versions(left: &[u64], right: &[u64]) -> Ordering {
    (0..left.len().max(right.len()))
        .map(|index| {
            left.get(index)
                .unwrap_or(&0)
                .cmp(right.get(index).unwrap_or(&0))
        })
        .find(|order| *order != Ordering::Equal)
        .unwrap_or(Ordering::Equal)
}

/// Compare digit runs numerically, including runs larger than u64. This also
/// keeps unknown providers/families deterministic across source response orders.
fn natural_compare(mut left: &str, mut right: &str) -> Ordering {
    while !left.is_empty() && !right.is_empty() {
        let left_number = left.as_bytes()[0].is_ascii_digit();
        let right_number = right.as_bytes()[0].is_ascii_digit();
        let end = |text: &str, number| {
            text.find(|ch: char| ch.is_ascii_digit() != number)
                .unwrap_or(text.len())
        };
        let left_end = end(left, left_number);
        let right_end = end(right, right_number);
        let left_part = &left[..left_end];
        let right_part = &right[..right_end];
        let order = if left_number && right_number {
            let left_part = left_part.trim_start_matches('0');
            let right_part = right_part.trim_start_matches('0');
            left_part
                .len()
                .cmp(&right_part.len())
                .then_with(|| left_part.cmp(right_part))
        } else {
            left_part.cmp(right_part)
        };
        if order != Ordering::Equal {
            return order;
        }
        left = &left[left_end..];
        right = &right[right_end..];
    }
    left.len().cmp(&right.len())
}

pub(super) fn normalize(provider_or_model: &str) -> String {
    provider_or_model.trim().to_ascii_lowercase()
}

pub(super) fn model_leaf(model_id: &str) -> &str {
    model_id.rsplit('/').next().unwrap_or(model_id)
}

pub(super) fn date_key(date_text: &str) -> Option<u32> {
    let date = NaiveDate::parse_from_str(date_text, "%Y-%m-%d")
        .ok()
        .or_else(|| {
            let (year, month) = date_text.split_once('-')?;
            if month.len() != 2 {
                return None;
            }
            NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, 1)
        })?;
    let year = u32::try_from(date.year()).ok()?;
    Some(year.saturating_mul(10_000) + date.month() * 100 + date.day())
}
