//! Presentation order for catalog models.
//!
//! Company and family ranking is display-only. It never rewrites a route id
//! and never decides whether a model can be served.

use chrono::{Datelike, NaiveDate};
use std::cmp::Ordering;
use std::collections::BTreeMap;

use super::{ModelMetadata, ModelMetadataCatalog};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FamilyOrder {
    newest_release: Option<u32>,
    catalog_release: Option<u32>,
    generation: Option<ModelGeneration>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ModelGeneration {
    prefix: String,
    version: Vec<u32>,
}

pub(super) fn family_order(
    catalog: &ModelMetadataCatalog,
    models: &[String],
) -> BTreeMap<(String, String), FamilyOrder> {
    let mut families = BTreeMap::new();
    for id in models {
        let Some(metadata) = catalog.resolve(id) else {
            continue;
        };
        let Some(key) = provider_key(metadata) else {
            continue;
        };
        let Some(family) = family_key(metadata) else {
            continue;
        };
        let release = metadata.release_date.as_deref().and_then(date_key);
        let generation = model_generation(id);
        families
            .entry((key, family))
            .and_modify(|order: &mut FamilyOrder| {
                order.newest_release = order.newest_release.max(release);
                order.catalog_release = order.newest_release;
                merge_generation(&mut order.generation, generation.clone());
            })
            .or_insert(FamilyOrder {
                newest_release: release,
                catalog_release: release,
                generation,
            });
    }

    // Sibling families in one numbered model generation form a catalog
    // cohort. Rank that cohort by its newest release, then order its
    // variants by their stable metadata family IDs. A newer sibling such
    // as GPT-6 Sol must not leapfrog GPT-6 Astra just because it launched
    // later.
    let mut cohort_releases = BTreeMap::new();
    for ((provider, _), order) in &families {
        let Some(generation) = &order.generation else {
            continue;
        };
        cohort_releases
            .entry((provider.clone(), generation.clone()))
            .and_modify(|release: &mut Option<u32>| {
                *release = (*release).max(order.newest_release);
            })
            .or_insert(order.newest_release);
    }
    for ((provider, _), order) in &mut families {
        if let Some(generation) = &order.generation {
            order.catalog_release = cohort_releases
                .get(&(provider.clone(), generation.clone()))
                .copied()
                .unwrap_or(order.newest_release);
        }
    }
    families
}

pub(super) fn compare_metadata(
    left_id: &str,
    left: Option<&ModelMetadata>,
    right_id: &str,
    right: Option<&ModelMetadata>,
    family_order: &BTreeMap<(String, String), FamilyOrder>,
) -> Ordering {
    let left_provider = presentation_provider(left_id, left);
    let right_provider = presentation_provider(right_id, right);

    match (left_provider.as_ref(), right_provider.as_ref()) {
        (Some(left_key), Some(right_key)) if left_key == right_key => {
            compare_output_kind(left_id, left, right_id, right)
                .then_with(|| {
                    artificial_analysis_rank(left_id).cmp(&artificial_analysis_rank(right_id))
                })
                .then_with(|| compare_families(left_key, left, right, family_order))
                .then_with(|| compare_model_dates(left, right))
        }
        (Some(left_key), Some(right_key)) => company_order(left_key)
            .cmp(&company_order(right_key))
            .then_with(|| left_key.cmp(right_key)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn presentation_provider(id: &str, metadata: Option<&ModelMetadata>) -> Option<String> {
    metadata
        .and_then(provider_key)
        .or_else(|| inferred_provider(id))
}

/// Display-only company for an id the metadata catalog does not know.
/// ChatGPT account models such as `codex-auto-review` still belong with the
/// other text models from that company, ahead of image generation.
fn inferred_provider(id: &str) -> Option<String> {
    let normalized = normalize(id);
    let model = catalog_model_leaf(strip_reasoning_effort(&normalized));
    let provider = if model.starts_with("gpt-")
        || model.starts_with("chatgpt-")
        || model.starts_with("codex-")
        || model.starts_with("dall-e")
    {
        "openai"
    } else if model.starts_with("claude-") {
        "anthropic"
    } else if model.starts_with("gemini-") {
        "google"
    } else if model.starts_with("grok-") {
        "xai"
    } else {
        return None;
    };
    Some(provider.to_string())
}

fn company_order(provider: &str) -> usize {
    match provider {
        "openai" => 0,
        "anthropic" => 1,
        "google" => 2,
        "xai" | "x-ai" => 3,
        _ => 4,
    }
}

fn family_key(metadata: &ModelMetadata) -> Option<String> {
    metadata
        .family
        .as_deref()
        .map(normalize)
        .filter(|family| !family.is_empty())
}

fn compare_families(
    provider: &str,
    left: Option<&ModelMetadata>,
    right: Option<&ModelMetadata>,
    families: &BTreeMap<(String, String), FamilyOrder>,
) -> Ordering {
    match (left.and_then(family_key), right.and_then(family_key)) {
        (Some(left), Some(right)) if left != right => {
            let left_order = &families[&(provider.to_string(), left.clone())];
            let right_order = &families[&(provider.to_string(), right.clone())];
            // Release dates are useful for versions within one family, but
            // they are not a stable ranking for sibling product families.
            // Anthropic can publish a newer Opus before a newer Fable while
            // the picker still needs to keep the product families together in
            // Relay's canonical order: Fable, Opus, Sonnet, Haiku. New or
            // provider-specific families remain after the known families and
            // continue to use the metadata date/ID tie-breakers below.
            compare_known_family_order(provider, &left, &right)
                .then_with(|| {
                    compare_optional_date_desc(
                        left_order.catalog_release,
                        right_order.catalog_release,
                    )
                })
                .then_with(|| left.cmp(&right))
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

fn compare_known_family_order(provider: &str, left: &str, right: &str) -> Ordering {
    let left_rank = known_family_rank(provider, left);
    let right_rank = known_family_rank(provider, right);
    match (left_rank, right_rank) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Stable product-family precedence used only where the provider exposes a
/// canonical tier order that release dates cannot represent. This deliberately
/// ranks family labels, not individual model IDs, so future versions inherit
/// the same placement automatically and unknown families remain discoverable.
fn known_family_rank(provider: &str, family: &str) -> Option<u8> {
    if provider != "anthropic" {
        return None;
    }
    let family = family.strip_prefix("claude-").unwrap_or(family);
    match family {
        "fable" => Some(0),
        "opus" => Some(1),
        "sonnet" => Some(2),
        "haiku" => Some(3),
        _ => None,
    }
}

/// Image-generation models stay after every text model from the same company.
/// A newer image release must not sit between chat families. Vision-capable
/// chat models still output text, so they keep their normal family position.
/// Same Artificial Analysis snapshot as the public catalog, 2026-09-29.
/// Text lines follow Intelligence Index and image lines follow Text-to-Image
/// Elo. The rank is the exact line after a reasoning-effort suffix is removed.
/// A shared family token is not enough: `gpt-5.6-sol` is not the `gpt-6-sol`
/// line. Unknown models compare as equal here and keep the existing family
/// and date order after known lines.
fn artificial_analysis_rank(model: &str) -> u32 {
    let model = catalog_model_leaf(strip_reasoning_effort(model));
    if model.starts_with("gpt-image-") || model.starts_with("dall-e") {
        return artificial_analysis_image_rank(model);
    }
    artificial_analysis_text_rank(model)
}

fn catalog_model_leaf(model: &str) -> &str {
    model.rsplit(['/', ':']).next().unwrap_or(model)
}

fn strip_reasoning_effort(model: &str) -> &str {
    const SUFFIXES: &[&str] = &[
        "-non-reasoning",
        "-xhigh",
        "-minimal",
        "-medium",
        "-high",
        "-low",
        "-max",
        "-none",
    ];
    for suffix in SUFFIXES {
        if let Some(stripped) = model.strip_suffix(suffix) {
            if !stripped.is_empty() && !stripped.ends_with('-') {
                return stripped;
            }
        }
    }
    model
}

fn artificial_analysis_text_rank(model: &str) -> u32 {
    match model {
        "gpt-6-astra" => 0,
        "gpt-6.1-sol" | "gpt-6-1-sol" => 1,
        "gpt-6-sol" => 2,
        "gpt-5.6-terra" => 3,
        "gpt-6-luna" => 4,
        _ => u32::MAX,
    }
}

fn artificial_analysis_image_rank(model: &str) -> u32 {
    match model {
        "gpt-image-2.5-sunburst" => 0,
        "gpt-image-2.5-flare" => 1,
        "gpt-image-2" => 2,
        _ => u32::MAX,
    }
}

fn compare_output_kind(
    left_id: &str,
    left: Option<&ModelMetadata>,
    right_id: &str,
    right: Option<&ModelMetadata>,
) -> Ordering {
    image_generation(left_id, left).cmp(&image_generation(right_id, right))
}

fn image_generation(id: &str, metadata: Option<&ModelMetadata>) -> bool {
    let Some(metadata) = metadata else {
        return image_id(id);
    };
    let outputs_image = metadata
        .capabilities
        .output_modalities
        .iter()
        .any(|modality| modality.eq_ignore_ascii_case("image"));
    let outputs_text = metadata
        .capabilities
        .output_modalities
        .iter()
        .any(|modality| modality.eq_ignore_ascii_case("text"));
    if outputs_image && !outputs_text {
        return true;
    }
    has_image_token(metadata.family.as_deref().unwrap_or(""))
        || has_image_token(metadata.name.as_deref().unwrap_or(""))
}

fn image_id(id: &str) -> bool {
    let normalized = normalize(id);
    let model = catalog_model_leaf(strip_reasoning_effort(&normalized));
    model.starts_with("gpt-image-") || model.starts_with("dall-e")
}

fn has_image_token(value: &str) -> bool {
    normalize(value)
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| matches!(token, "image" | "dalle" | "dall"))
}

fn merge_generation(current: &mut Option<ModelGeneration>, incoming: Option<ModelGeneration>) {
    match (current.as_ref(), incoming) {
        (Some(existing), Some(incoming)) if existing.prefix == incoming.prefix => {
            if compare_generation_version(&incoming.version, &existing.version) == Ordering::Greater
            {
                *current = Some(incoming);
            }
        }
        (None, None) => {}
        _ => *current = None,
    }
}

fn model_generation(id: &str) -> Option<ModelGeneration> {
    let normalized = normalize(id);
    let leaf = model_leaf(&normalized);
    let start = leaf.find(|character: char| character.is_ascii_digit())?;
    let prefix = leaf[..start].trim_end_matches(['-', '_', '.']);
    if prefix.is_empty() {
        return None;
    }

    let version_end = leaf[start..]
        .char_indices()
        .take_while(|(offset, character)| {
            character.is_ascii_digit()
                || (*character == '.'
                    && leaf[start + offset + 1..]
                        .chars()
                        .next()
                        .is_some_and(|next| next.is_ascii_digit()))
        })
        .map(|(offset, character)| offset + character.len_utf8())
        .last()?;
    let version = leaf[start..start + version_end]
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (!version.is_empty()).then(|| ModelGeneration {
        prefix: prefix.to_owned(),
        version,
    })
}

fn compare_generation_version(left: &[u32], right: &[u32]) -> Ordering {
    let width = left.len().max(right.len());
    (0..width)
        .map(|index| {
            left.get(index)
                .copied()
                .unwrap_or_default()
                .cmp(&right.get(index).copied().unwrap_or_default())
        })
        .find(|ordering| *ordering != Ordering::Equal)
        .unwrap_or(Ordering::Equal)
}

fn compare_model_dates(left: Option<&ModelMetadata>, right: Option<&ModelMetadata>) -> Ordering {
    compare_optional_date_desc(
        metadata_date(left, |metadata| metadata.release_date.as_deref()),
        metadata_date(right, |metadata| metadata.release_date.as_deref()),
    )
    .then_with(|| {
        compare_optional_date_desc(
            metadata_date(left, |metadata| metadata.last_updated.as_deref()),
            metadata_date(right, |metadata| metadata.last_updated.as_deref()),
        )
    })
}

fn compare_optional_date_desc(left: Option<u32>, right: Option<u32>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn metadata_date(
    metadata: Option<&ModelMetadata>,
    field: impl FnOnce(&ModelMetadata) -> Option<&str>,
) -> Option<u32> {
    metadata.and_then(field).and_then(date_key)
}

fn provider_key(metadata: &ModelMetadata) -> Option<String> {
    let provider = normalize(&metadata.provider);
    (!provider.is_empty()).then_some(provider)
}

pub(super) fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

pub(super) fn model_leaf(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

pub(super) fn date_key(value: &str) -> Option<u32> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .or_else(|| {
            let (year, month) = value.split_once('-')?;
            if month.len() != 2 {
                return None;
            }
            NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, 1)
        })?;
    let year = u32::try_from(date.year()).ok()?;
    Some(year.saturating_mul(10_000) + date.month() * 100 + date.day())
}
