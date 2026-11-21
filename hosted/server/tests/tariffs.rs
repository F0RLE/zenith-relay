use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
use zenith_relay_hosted::accounting::{
    ApprovedPurchase, Component::*, Offer, PricingError::*, Rate, Rates, SaleRule,
};

fn rate(s: &str) -> Rate {
    s.parse().unwrap()
}

fn components(input: &str, output: &str, read: &str) -> Rates {
    [
        (Input, rate(input)),
        (Output, rate(output)),
        (CacheRead, rate(read)),
    ]
    .into()
}

#[test]
fn seller_previews_matching_complete_proposals_and_keeps_purchase_separate() {
    let variant = Uuid::new_v4();
    let mut a = ApprovedPurchase {
        variant,
        connection: Uuid::new_v4(),
        purchase_revision: 4,
        default_markup: None,
        rates: components("2", "3", "0.50"),
    };
    let offer = Offer::text(variant, [Input, Output, CacheRead].into()).unwrap();
    let preview = |rule, src: &[ApprovedPurchase]| offer.preview(&rule, src);
    assert_eq!(
        preview(SaleRule::ApiDefault, std::slice::from_ref(&a)),
        Err(NoProposal)
    );
    a.default_markup = Some((5, rate("0")));
    assert_eq!(
        preview(SaleRule::ApiDefault, std::slice::from_ref(&a))
            .unwrap()
            .selling,
        a.rates
    );
    a.default_markup = Some((6, rate("25")));
    let b = ApprovedPurchase {
        variant,
        connection: Uuid::new_v4(),
        purchase_revision: 7,
        default_markup: Some((8, rate("10"))),
        rates: components("1", "6", "0.25"),
    };
    let (a_id, b_id) = (a.connection, b.connection);
    let mut sources = [a, b];
    let inherited = preview(SaleRule::ApiDefault, &sources).unwrap();
    assert_eq!(inherited.selling, components("2.5", "6.6", "0.625"));
    assert_eq!(
        inherited.inputs,
        [(a_id, (4, Some(6))), (b_id, (7, Some(8)))].into()
    );
    sources.swap(0, 1);
    assert_eq!(preview(SaleRule::ApiDefault, &sources).unwrap(), inherited);
    let custom = preview(SaleRule::ModelMarkup(rate("50")), &sources).unwrap();
    assert_eq!(custom.selling, components("3", "9", "0.75"));
    assert_eq!(custom.inputs, [(a_id, (4, None)), (b_id, (7, None))].into());
    let fixed = components("3", "5", "0.40");
    let mut partial = fixed.clone();
    partial.remove(&Output);
    assert_eq!(
        preview(SaleRule::Fixed(partial), &sources),
        Err(MissingSalePrice)
    );
    let review = preview(SaleRule::Fixed(fixed.clone()), &sources).unwrap();
    assert_eq!(review.selling, fixed);
    assert!(review.inputs.is_empty());
    assert_eq!(
        review.below_cost,
        [(a_id, [CacheRead].into()), (b_id, [Output].into())].into()
    );
    sources[0].variant = Uuid::new_v4();
    assert_eq!(
        preview(SaleRule::ApiDefault, &sources).unwrap().selling,
        components("2.5", "3.75", "0.625")
    );
    sources[0].variant = variant;
    sources[0].rates.remove(&Output);
    assert_eq!(
        preview(SaleRule::ApiDefault, &sources)
            .unwrap()
            .inputs
            .len(),
        1
    );
    sources[0].connection = a_id;
    assert_eq!(preview(SaleRule::ApiDefault, &sources), Err(InvalidSource));
    assert_eq!(inherited.selling, components("2.5", "6.6", "0.625"));
}

#[test]
fn exact_rate_round_trip_and_single_final_rounding_match_the_web_vectors() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/retail-rates.json")).unwrap();
    let offer = Offer::text(Uuid::new_v4(), [Input, Output].into()).unwrap();
    for row in fixture["valid"].as_array().unwrap() {
        let rate_in: Rate = serde_json::from_value(row["input"].clone()).unwrap();
        assert_eq!(json!(rate_in), row["wire"]);
        let count = row["tokens"].as_str().unwrap().parse::<i64>().unwrap();
        let result = offer.value_micro_usd(
            &[(Input, rate_in), (Output, rate("0"))].into(),
            &[(Input, count), (Output, 0)].into(),
        );
        match row["micro_usd"].as_str() {
            Some(n) => assert_eq!(result, Ok(n.parse().unwrap())),
            None => assert_eq!(result, Err(ChargeOverflow)),
        }
    }
    for invalid in fixture["invalid"].as_array().unwrap() {
        assert!(serde_json::from_value::<Rate>(invalid.clone()).is_err());
    }
    let rates = [(Input, rate("0.4")), (Output, rate("0.6"))].into();
    assert_eq!(
        offer.value_micro_usd(&rates, &[(Input, 1), (Output, 1)].into()),
        Ok(1)
    );
    assert_eq!(
        offer.value_micro_usd(&rates, &[(Input, 1)].into()),
        Err(InvalidUsage)
    );
    assert_eq!(
        offer.value_micro_usd(&rates, &[(Input, -1), (Output, 0)].into()),
        Err(InvalidUsage)
    );
    assert_eq!(
        offer.value_micro_usd(&BTreeMap::new(), &BTreeMap::new()),
        Err(MissingSalePrice)
    );
}

#[test]
fn precision_failures_and_overlapping_cache_components_do_not_become_prices() {
    let id = Uuid::new_v4();
    assert!(Offer::text(id, [Input, Output, CacheWrite, CacheWrite1h].into()).is_err());
    let required = BTreeSet::from([Input, Output, CacheRead, CacheWrite5m, CacheWrite1h]);
    let offer = Offer::text(id, required.clone()).unwrap();
    let mut src = ApprovedPurchase {
        variant: id,
        connection: Uuid::new_v4(),
        purchase_revision: 1,
        default_markup: None,
        rates: required.iter().map(|c| (*c, rate("0"))).collect(),
    };
    src.rates.insert(CacheWrite5m, rate("1.5"));
    src.rates.insert(CacheWrite1h, rate("2"));
    assert_eq!(
        offer.value_micro_usd(
            &src.rates,
            &[
                (Input, 100),
                (Output, 2),
                (CacheRead, 10),
                (CacheWrite5m, 2),
                (CacheWrite1h, 4)
            ]
            .into()
        ),
        Ok(11)
    );
    src.rates.insert(Input, rate("0.000000000001"));
    let preview =
        |markup, s: &[ApprovedPurchase]| offer.preview(&SaleRule::ModelMarkup(rate(markup)), s);
    assert_eq!(
        preview("0.01", std::slice::from_ref(&src)),
        Err(UnrepresentableRate)
    );
    src.rates
        .insert(Input, rate("999999999999999999.999999999999"));
    assert_eq!(
        preview("0", std::slice::from_ref(&src)).unwrap().selling,
        src.rates
    );
    assert_eq!(
        preview("100", std::slice::from_ref(&src)),
        Err(UnrepresentableRate)
    );
}
