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
    pub fn normalize(subscription_input: SubscriptionInput) -> Self {
        let plan_type = subscription_input
            .plan_type
            .map(|plan_name| normalize_subscription_plan(&plan_name))
            .filter(|normalized_plan| !normalized_plan.is_empty());
        let status = if subscription_input.forbidden {
            SubscriptionStatus::Forbidden
        } else if subscription_input
            .active_until_ms
            .is_some_and(|active_until| active_until <= subscription_input.observed_at_ms)
        {
            SubscriptionStatus::Expired
        } else if plan_type.is_some() || subscription_input.active_until_ms.is_some() {
            SubscriptionStatus::Active
        } else {
            SubscriptionStatus::Unknown
        };
        Self {
            plan_type,
            active_until_ms: subscription_input.active_until_ms,
            status,
            updated_at_ms: Some(subscription_input.observed_at_ms),
        }
    }

    pub fn is_free_plan(&self) -> bool {
        self.plan_type
            .as_deref()
            .is_some_and(|plan| normalize_subscription_plan(plan) == "free")
    }
}

pub(crate) fn normalize_subscription_plan(plan_name: &str) -> String {
    let normalized_plan = plan_name.trim().to_ascii_lowercase();
    let compact = normalized_plan
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
        _ => normalized_plan,
    }
}
