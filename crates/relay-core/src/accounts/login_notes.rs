/// Optional sign-in notes stored with an account credential.
/// Empty or malformed values are dropped. Callers must not log the input.
pub fn normalize_login_email(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() < 3
        || value.len() > 320
        || value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return None;
    }
    let (local, domain) = value.split_once('@')?;
    if local.is_empty() || domain.is_empty() || domain.contains('@') || local.contains('@') {
        return None;
    }
    Some(value.to_string())
}

pub fn normalize_login_phone(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 32
        || !value.chars().any(|character| character.is_ascii_digit())
        || !value.chars().all(|character| {
            character.is_ascii_digit() || matches!(character, '+' | '-' | ' ' | '(' | ')')
        })
    {
        return None;
    }
    Some(value.to_string())
}

pub fn normalize_login_password(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 1_024 || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.to_string())
}

pub fn normalize_login_totp_secret(value: &str) -> Option<String> {
    let compact: String = value
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
