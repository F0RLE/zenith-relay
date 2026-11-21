use super::estimate::{component_value, sum_component_values};
use super::{
    ApiEquivalentBreakdown, ApiEquivalentSummary, ApiEquivalentUsage, CatalogPriceResolver,
    UsageContextBand, UsagePriceClass,
};
use crate::{pricing::PriceSource, usage::ImageUsage};
use serde::{Deserialize, Serialize};

/// Price-independent units retained by hosts, so catalog refresh can revalue history.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImagePriceKind {
    InputText,
    CachedText,
    InputImage,
    CachedImage,
    OutputText,
    OutputImage,
    SvgInput,
    SvgOutput,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImagePriceComponent {
    pub kind: ImagePriceKind,
    pub model: String,
    pub tokens: u64,
    pub images: u64,
    pub price_class: String,
    pub context_band: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageEquivalentBreakdown {
    pub input_text: Option<ApiEquivalentSummary>,
    pub input_image: Option<ApiEquivalentSummary>,
    pub output_text: Option<ApiEquivalentSummary>,
    pub output_image: Option<ApiEquivalentSummary>,
    pub svg_input: Option<ApiEquivalentSummary>,
    pub svg_output: Option<ApiEquivalentSummary>,
}

/// Never assign a text rate to unsplit image tokens. Inconsistent modality or
/// cache snapshots stay unpriced; unknown cache counts do not invent a discount.
pub fn image_price_components(
    model: &str,
    usage: ApiEquivalentUsage,
    image: &ImageUsage,
) -> Vec<ImagePriceComponent> {
    use ImagePriceKind::*;
    let mut parts = Vec::with_capacity(8);
    let mut push = |kind, model: &str, tokens, images, band: UsageContextBand| {
        parts.push(ImagePriceComponent {
            kind,
            model: model.to_owned(),
            tokens,
            images,
            price_class: usage.price_class.as_label().to_owned(),
            context_band: band.as_label().to_owned(),
        });
    };
    let svg_input = image.svg_usage.as_ref().and_then(|svg| svg.input_tokens);
    let svg_output = image.svg_usage.as_ref().and_then(|svg| svg.output_tokens);
    let input_budget = usage
        .input_tokens
        .map(|total| total.saturating_sub(svg_input.unwrap_or_default()));
    let output_budget = usage
        .output_tokens
        .map(|total| total.saturating_sub(svg_output.unwrap_or_default()));
    let fits = |a: Option<u64>, b: Option<u64>, budget: Option<u64>| {
        a.unwrap_or_default()
            .checked_add(b.unwrap_or_default())
            .is_some_and(|sum| budget.is_none_or(|budget| sum <= budget))
    };
    if image.input_details_consistent != Some(false)
        && fits(
            image.input_text_tokens,
            image.input_image_tokens,
            input_budget,
        )
        && let Some((text_cache, image_cache)) = cache_split(image)
    {
        for (tokens, cached, ordinary_kind, cached_kind) in [
            (image.input_text_tokens, text_cache, InputText, CachedText),
            (
                image.input_image_tokens,
                image_cache,
                InputImage,
                CachedImage,
            ),
        ] {
            if let Some(tokens) = tokens {
                push(
                    ordinary_kind,
                    model,
                    tokens - cached,
                    0,
                    UsageContextBand::Base,
                );
                if cached > 0 {
                    push(cached_kind, model, cached, 0, UsageContextBand::Base);
                }
            }
        }
    }
    if image.output_details_consistent != Some(false)
        && fits(
            image.output_text_tokens,
            image.output_image_tokens,
            output_budget,
        )
    {
        if let Some(tokens) = image.output_text_tokens {
            push(OutputText, model, tokens, 0, UsageContextBand::Base);
        }
        // A per-image schedule can price a completed image without token counts.
        if image.output_image_tokens.is_some() || image.generated_images > 0 {
            push(
                OutputImage,
                model,
                image.output_image_tokens.unwrap_or_default(),
                image.generated_images,
                UsageContextBand::Base,
            );
        }
    }
    if let Some(svg) = &image.svg_usage {
        let band = UsageContextBand::from_input_tokens(svg.input_tokens);
        for (kind, tokens, budget) in [
            (SvgInput, svg.input_tokens, usage.input_tokens),
            (SvgOutput, svg.output_tokens, usage.output_tokens),
        ] {
            if let Some(tokens) =
                tokens.filter(|tokens| budget.is_none_or(|budget| *tokens <= budget))
            {
                push(
                    kind,
                    svg.model.as_deref().unwrap_or_default(),
                    tokens,
                    0,
                    band,
                );
            }
        }
    }
    parts
}

fn cache_split(image: &ImageUsage) -> Option<(u64, u64)> {
    let (text, picture) = (
        image.input_text_tokens.unwrap_or_default(),
        image.input_image_tokens.unwrap_or_default(),
    );
    let (mut a, mut b) = (image.cached_text_tokens, image.cached_image_tokens);
    if let Some(total) = image.cached_input_tokens {
        match (a, b) {
            (Some(x), Some(y)) if x.checked_add(y)? != total => return None,
            (Some(x), None) => b = Some(total.checked_sub(x)?),
            (None, Some(y)) => a = Some(total.checked_sub(y)?),
            (None, None) if total == 0 => (),
            (None, None) if image.input_image_tokens == Some(0) => a = Some(total),
            (None, None) if image.input_text_tokens == Some(0) => b = Some(total),
            (None, None) => return None,
            _ => (),
        }
    }
    let (a, b) = (a.unwrap_or_default(), b.unwrap_or_default());
    (a <= text && b <= picture).then_some((a, b))
}

impl CatalogPriceResolver<'_> {
    pub fn estimate_image_component(
        &self,
        kind: &str,
        id: &str,
        part: &ImagePriceComponent,
    ) -> (ApiEquivalentSummary, PriceSource) {
        use ImagePriceKind::*;
        if matches!(part.kind, SvgInput | SvgOutput) {
            let usage = ApiEquivalentUsage {
                input_tokens: (part.kind == SvgInput).then_some(part.tokens),
                output_tokens: (part.kind == SvgOutput).then_some(part.tokens),
                total_tokens: Some(part.tokens),
                price_class: UsagePriceClass::from_label(&part.price_class),
                context_band: UsageContextBand::from_label(&part.context_band),
                ..Default::default()
            };
            return (
                self.estimate(kind, id, Some(&part.model), usage),
                self.source(kind, id, Some(&part.model)),
            );
        }
        let (quote, source) =
            self.context
                .candidate_image_price(self.catalog, kind, id, &part.model);
        // Image catalog fields currently describe Standard; do not borrow Batch
        // or text-model tier schedules for an unsupported image service tier.
        let quote = quote.filter(|_| {
            UsagePriceClass::from_label(&part.price_class) == UsagePriceClass::Standard
        });
        let rate = quote.and_then(|q| match part.kind {
            InputText => q.input_text_micro_usd_per_million,
            CachedText => q.cached_text_micro_usd_per_million,
            InputImage => q.input_micro_usd_per_image_token,
            CachedImage => q.cached_image_micro_usd_per_million,
            OutputText => q.output_text_micro_usd_per_million,
            OutputImage => q.output_micro_usd_per_image_token,
            SvgInput | SvgOutput => None,
        });
        if part.kind == OutputImage
            && rate.is_none()
            && part.images > 0
            && let Some(price) = quote.and_then(|q| q.output_micro_usd_per_image)
        {
            return (
                ApiEquivalentSummary {
                    micro_usd: price.saturating_mul(part.images),
                    priced_tokens: part.tokens,
                    unpriced_tokens: 0,
                },
                source,
            );
        }
        (
            component_value(Some(part.tokens), rate).unwrap_or_default(),
            if rate.is_some() {
                source
            } else {
                PriceSource::Unpriced
            },
        )
    }

    pub fn estimate_image_breakdown(
        &self,
        kind: &str,
        id: &str,
        model: Option<&str>,
        usage: ApiEquivalentUsage,
        image: &ImageUsage,
    ) -> (ApiEquivalentSummary, ApiEquivalentBreakdown) {
        use ImagePriceKind::*;
        let mut detail = ImageEquivalentBreakdown::default();
        let mut cache_read = None;
        let mut uncached_input = None;
        let mut measured = 0_u64;
        for part in image_price_components(model.unwrap_or_default(), usage, image) {
            measured = measured.saturating_add(part.tokens);
            let value = Some(self.estimate_image_component(kind, id, &part).0);
            let target = match part.kind {
                InputText | CachedText => &mut detail.input_text,
                InputImage | CachedImage => &mut detail.input_image,
                OutputText => &mut detail.output_text,
                OutputImage => &mut detail.output_image,
                SvgInput => &mut detail.svg_input,
                SvgOutput => &mut detail.svg_output,
            };
            *target = sum_component_values([*target, value]);
            if matches!(part.kind, CachedText | CachedImage) {
                cache_read = sum_component_values([cache_read, value]);
            }
            if matches!(part.kind, InputText | InputImage | SvgInput) {
                uncached_input = sum_component_values([uncached_input, value]);
            }
        }
        let input = sum_component_values([detail.input_text, detail.input_image, detail.svg_input]);
        let output =
            sum_component_values([detail.output_text, detail.output_image, detail.svg_output]);
        let measured = measured.max(
            usage
                .input_tokens
                .unwrap_or_default()
                .saturating_add(usage.output_tokens.unwrap_or_default()),
        );
        let mut summary = sum_component_values([input, output]).unwrap_or_default();
        summary.unpriced_tokens = usage
            .total_tokens
            .unwrap_or(measured)
            .max(measured)
            .saturating_sub(summary.priced_tokens);
        (
            summary,
            ApiEquivalentBreakdown {
                input,
                output,
                uncached_input,
                cache_read,
                cache_write: None,
                image: Some(detail),
            },
        )
    }
}

#[cfg(test)]
mod tests;
