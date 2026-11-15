pub(super) const JSON: &str = concat!(
    "[",
    include_str!("auth_page_patterns/en.json"),
    ",",
    include_str!("auth_page_patterns/ru.json"),
    "]"
);

#[cfg(test)]
mod tests {
    use super::JSON;

    #[test]
    fn embedded_locales_have_complete_nonempty_patterns() {
        let locales: Vec<serde_json::Value> = serde_json::from_str(JSON).unwrap();
        assert_eq!(locales.len(), 2);
        for locale in locales {
            for key in ["loginTitle", "loginPrimaryAction", "unsafeSubmit"] {
                let phrases = locale[key].as_array().unwrap();
                assert!(!phrases.is_empty());
                assert!(phrases.iter().all(|phrase| {
                    phrase
                        .as_str()
                        .is_some_and(|value| !value.trim().is_empty())
                }));
            }
        }
    }
}
