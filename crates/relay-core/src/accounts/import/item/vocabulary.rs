pub(in crate::accounts::import::item) fn is_oauth_mode(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "chatgpt" | "oauth" | "openai_oauth"
    )
}

pub(in crate::accounts::import::item) fn is_openai_platform(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "openai" | "chatgpt" | "codex"
    )
}

pub(in crate::accounts::import::item) fn is_recognized_auth_mode(value: &str) -> bool {
    is_token_mode(value) || is_api_key_mode(value)
}

pub(in crate::accounts::import::item) fn is_agent_identity_mode(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "agentidentity" | "agent_identity"
    )
}

pub(in crate::accounts::import::item) fn is_api_key_mode(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "apikey" | "api_key"
    )
}

pub(in crate::accounts::import::item) fn is_token_mode(value: &str) -> bool {
    is_oauth_mode(value)
        || is_agent_identity_mode(value)
        || matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "token"
                | "imported_token"
                | "personal_access_token"
                | "personalaccesstoken"
                | "access_token"
                | "accesstoken"
                | "apikey"
                | "api_key"
        )
}

pub(in crate::accounts::import::item) const ACCESS_TOKEN_FIELDS: &[&str] = &[
    "access_token",
    "accessToken",
    "personal_access_token",
    "personalAccessToken",
    "at_token",
    "atToken",
];
pub(in crate::accounts::import::item) const REFRESH_TOKEN_FIELDS: &[&str] =
    &["refresh_token", "refreshToken"];
pub(in crate::accounts::import::item) const ID_TOKEN_FIELDS: &[&str] = &["id_token", "idToken"];
pub(in crate::accounts::import::item) const API_KEY_FIELDS: &[&str] =
    &["OPENAI_API_KEY", "openai_api_key", "api_key", "apiKey"];
pub(in crate::accounts::import::item) const AGENT_PRIVATE_KEY_FIELDS: &[&str] =
    &["agent_private_key", "agentPrivateKey"];
pub(in crate::accounts::import::item) const AGENT_RUNTIME_ID_FIELDS: &[&str] =
    &["agent_runtime_id", "agentRuntimeId"];
pub(in crate::accounts::import::item) const AGENT_TASK_ID_FIELDS: &[&str] = &["task_id", "taskId"];
pub(in crate::accounts::import::item) const EMAIL_FIELDS: &[&str] =
    &["email", "identity_email", "account_email", "accountEmail"];
pub(in crate::accounts::import::item) const PHONE_FIELDS: &[&str] =
    &["phone", "phone_number", "phoneNumber", "telephone"];
pub(in crate::accounts::import::item) const PASSWORD_FIELDS: &[&str] = &["password", "Password"];
pub(in crate::accounts::import::item) const TOTP_SECRET_FIELDS: &[&str] = &[
    "2fa",
    "totp",
    "totp_secret",
    "totpSecret",
    "otp_secret",
    "otpSecret",
];
pub(in crate::accounts::import::item) const ACCOUNT_ID_FIELDS: &[&str] = &[
    "chatgpt_account_id",
    "chatgptAccountId",
    "account_id",
    "accountId",
    "workspace_id",
    "workspaceId",
];
pub(in crate::accounts::import::item) const USER_ID_FIELDS: &[&str] =
    &["chatgpt_user_id", "chatgptUserId", "user_id", "userId"];
pub(in crate::accounts::import::item) const ORGANIZATION_ID_FIELDS: &[&str] = &[
    "organization_id",
    "organizationId",
    "org_id",
    "orgId",
    "poid",
    "POID",
];
pub(in crate::accounts::import::item) const TAG_FIELDS: &[&str] = &["tags", "Tags"];
pub(in crate::accounts::import::item) const PLAN_FIELDS: &[&str] = &[
    "chatgpt_plan_type",
    "chatgptPlanType",
    "plan_type",
    "planType",
    "auth_file_plan_type",
    "authFilePlanType",
    "plan",
];
pub(in crate::accounts::import::item) const AUTH_MODE_FIELDS: &[&str] = &[
    "auth_mode",
    "authMode",
    "authType",
    "openai_auth_mode",
    "openaiAuthMode",
];
pub(in crate::accounts::import::item) const EXPIRES_AT_FIELDS: &[&str] =
    &["expires_at", "expiresAt", "expired"];
pub(in crate::accounts::import::item) const SUBSCRIPTION_EXPIRES_AT_FIELDS: &[&str] = &[
    "subscription_expires_at",
    "subscriptionExpiresAt",
    "subscription_active_until",
    "subscriptionActiveUntil",
    "chatgpt_subscription_active_until",
    "chatgptSubscriptionActiveUntil",
];
