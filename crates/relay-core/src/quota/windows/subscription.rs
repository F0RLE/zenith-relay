use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStatus {
    #[default]
    Unknown,
    Active,
    Expired,
    Forbidden,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionInput {
    pub plan_type: Option<String>,
    pub active_until_ms: Option<u64>,
    pub forbidden: bool,
    pub observed_at_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub plan_type: Option<String>,
    pub active_until_ms: Option<u64>,
    pub status: SubscriptionStatus,
    pub updated_at_ms: Option<u64>,
}

impl Subscription {
    pub fn normalize(input: SubscriptionInput) -> Self {
        let plan_type = input
            .plan_type
            .map(|value| normalize_subscription_plan(&value))
            .filter(|value| !value.is_empty());
        let status = if input.forbidden {
            SubscriptionStatus::Forbidden
        } else if input
            .active_until_ms
            .is_some_and(|active_until| active_until <= input.observed_at_ms)
        {
            SubscriptionStatus::Expired
        } else if plan_type.is_some() || input.active_until_ms.is_some() {
            SubscriptionStatus::Active
        } else {
            SubscriptionStatus::Unknown
        };
        Self {
            plan_type,
            active_until_ms: input.active_until_ms,
            status,
            updated_at_ms: Some(input.observed_at_ms),
        }
    }

    pub fn is_free_plan(&self) -> bool {
        self.plan_type
            .as_deref()
            .is_some_and(|plan| normalize_subscription_plan(plan) == "free")
    }
}

pub(crate) fn normalize_subscription_plan(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    let compact = value
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(char::from)
        .collect::<String>();
    match compact.as_str() {
        "free" | "freeplan" | "freetier" | "chatgptfree" | "chatgptfreeplan"
        | "chatgptfreetier" => "free".to_string(),
        "plus" | "plusplan" | "chatgptplus" | "chatgptplusplan" => "plus".to_string(),
        "pro" | "proplan" | "chatgptpro" | "chatgptproplan" => "pro".to_string(),
        "business"
        | "businessplan"
        | "team"
        | "teamplan"
        | "chatgptbusiness"
        | "chatgptbusinessplan"
        | "chatgptteam"
        | "chatgptteamplan" => "business".to_string(),
        "enterprise" | "enterpriseplan" | "chatgptenterprise" | "chatgptenterpriseplan" => {
            "enterprise".to_string()
        }
        _ => value,
    }
}
