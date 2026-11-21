use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

const MAX_ROWS: usize = 8192;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const HEADER: &str = concat!(
    "request_id,started_at_utc,model,key_prefix,execution,accounting,charge_basis,tariff_revision,",
    "input_tokens,cached_input_tokens,output_tokens,total_tokens,usage_quality,",
    "hold_micro_usd,original_charge_micro_usd,posted_credit_micro_usd,net_charge_micro_usd,currency,",
    "report_from_utc,report_to_utc,report_as_of_utc,coverage\r\n"
);

#[derive(Deserialize)]
struct Report {
    totals: Totals,
    items: Vec<Receipt>,
    from_utc: String,
    to_utc: String,
    as_of_utc: String,
}

#[derive(Deserialize)]
struct Totals {
    requests: usize,
}

// Export is an allowlist, not a serialization of the receipt or key objects.
#[derive(Deserialize)]
struct Receipt {
    request_id: Uuid,
    started_at_utc: String,
    model: String,
    key_id: Uuid,
    execution: String,
    accounting: String,
    basis: Option<String>,
    tariff_revision: String,
    input_tokens: Option<String>,
    cached_tokens: Option<String>,
    output_tokens: Option<String>,
    total_tokens: Option<String>,
    usage_quality: String,
    hold_micro_usd: String,
    charge_micro_usd: Option<String>,
    credit: Option<Credit>,
}

#[derive(Deserialize)]
struct Credit {
    amount_micro_usd: String,
}

pub(super) fn render(value: Value) -> Option<Vec<u8>> {
    let report: Report = serde_json::from_value(value).ok()?;
    if report.items.len() > MAX_ROWS || report.totals.requests != report.items.len() {
        return None;
    }
    let mut csv = String::from(HEADER);
    for r in report.items {
        let prefix = format!("rlk_{}", &r.key_id.simple().to_string()[..12]);
        let original = r.charge_micro_usd.as_deref().unwrap_or_default();
        let credit = r.credit.as_ref().map(|c| c.amount_micro_usd.as_str());
        let net = match r.charge_micro_usd {
            Some(ref amount) => amount
                .parse::<u64>()
                .ok()?
                .checked_sub(credit.unwrap_or("0").parse().ok()?)?
                .to_string(),
            None if credit.is_none() => String::new(),
            None => return None,
        };
        let row: [&str; 22] = [
            &r.request_id.to_string(),
            &r.started_at_utc,
            &r.model,
            &prefix,
            &r.execution,
            &r.accounting,
            r.basis.as_deref().unwrap_or_default(),
            &r.tariff_revision,
            r.input_tokens.as_deref().unwrap_or_default(),
            r.cached_tokens.as_deref().unwrap_or_default(),
            r.output_tokens.as_deref().unwrap_or_default(),
            r.total_tokens.as_deref().unwrap_or_default(),
            &r.usage_quality,
            &r.hold_micro_usd,
            original,
            credit.unwrap_or_default(),
            &net,
            "USD",
            &report.from_utc,
            &report.to_utc,
            &report.as_of_utc,
            "complete",
        ];
        for (i, value) in row.into_iter().enumerate() {
            if i > 0 {
                csv.push(',');
            }
            field(&mut csv, value);
        }
        csv.push_str("\r\n");
        if csv.len() > MAX_BYTES {
            return None;
        }
    }
    Some(csv.into_bytes())
}

fn field(out: &mut String, value: &str) {
    let formula = value.starts_with(['\t', '\r', '\n'])
        || value
            .trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
            .starts_with(['=', '+', '-', '@', '＝', '＋', '－', '＠']);
    if formula || value.contains([',', '"', '\n', '\r']) {
        out.push('"');
        if formula {
            out.push('\'');
        }
        for c in value.chars() {
            if c == '"' {
                out.push('"');
            }
            out.push(c);
        }
        out.push('"');
    } else {
        out.push_str(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_escape_csv_and_neutralize_spreadsheet_formulas() {
        for (val, expected) in [
            ("model:v1", "model:v1"),
            ("  =1+1", "\"'  =1+1\""),
            ("\tplain", "\"'\tplain\""),
            ("＋123", "\"'＋123\""),
            ("@SUM(1,2)", "\"'@SUM(1,2)\""),
            ("a,\"b\"\nc", "\"a,\"\"b\"\"\nc\""),
        ] {
            let mut out = String::new();
            field(&mut out, val);
            assert_eq!(out, expected);
        }
    }
}
