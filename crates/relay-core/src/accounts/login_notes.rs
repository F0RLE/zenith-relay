/// Optional sign-in notes stored with an account credential.
/// Empty or malformed values are dropped. Callers must not log the input.
pub fn normalize_login_email(email_text: &str) -> Option<String> {
    let trimmed_email = email_text.trim();
    if trimmed_email.len() < 3
        || trimmed_email.len() > 320
        || trimmed_email
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return None;
    }
    let (local, domain) = trimmed_email.split_once('@')?;
    if local.is_empty() || domain.is_empty() || domain.contains('@') || local.contains('@') {
        return None;
    }
    Some(trimmed_email.to_string())
}

pub fn normalize_login_phone(phone_text: &str) -> Option<String> {
    let trimmed_phone = phone_text.trim();
    if trimmed_phone.is_empty()
        || trimmed_phone.len() > 32
        || !trimmed_phone
            .chars()
            .any(|character| character.is_ascii_digit())
        || !trimmed_phone.chars().all(|character| {
            character.is_ascii_digit() || matches!(character, '+' | '-' | ' ' | '(' | ')')
        })
    {
        return None;
    }
    Some(trimmed_phone.to_string())
}

pub fn normalize_login_password(password_text: &str) -> Option<String> {
    let trimmed_password = password_text.trim();
    if trimmed_password.is_empty()
        || trimmed_password.len() > 1_024
        || trimmed_password.chars().any(char::is_control)
    {
        return None;
    }
    Some(trimmed_password.to_string())
}

pub fn normalize_login_totp_secret(totp_text: &str) -> Option<String> {
    let compact: String = totp_text
        .chars()
        .filter(|character| !character.is_whitespace() && *character != '-')
        .collect();
    let compact = compact.trim_end_matches('=').to_ascii_uppercase();
    if !(8..=128).contains(&compact.len())
        || !compact
            .chars()
            .all(|character| matches!(character, 'A'..='Z' | '2'..='7'))
    {
        return None;
    }
    Some(compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_notes_keep_present_values_and_drop_malformed_ones() {
        assert_eq!(
            normalize_login_email(" person@example.test "),
            Some("person@example.test".to_string())
        );
        assert_eq!(normalize_login_email("not-an-email"), None);
        assert_eq!(
            normalize_login_phone(" 950-000-000 "),
            Some("950-000-000".to_string())
        );
        assert_eq!(normalize_login_phone("phone"), None);
        assert_eq!(
            normalize_login_password(" synthetic-password "),
            Some("synthetic-password".to_string())
        );
        assert_eq!(normalize_login_password("bad\npassword"), None);
        assert_eq!(
            normalize_login_totp_secret("gezd gnbv-gy3t qojq"),
            Some("GEZDGNBVGY3TQOJQ".to_string())
        );
        assert_eq!(normalize_login_totp_secret("11111111"), None);
    }
}
