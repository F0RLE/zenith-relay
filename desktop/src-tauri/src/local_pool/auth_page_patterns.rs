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
                assert!(locale[key].as_str().is_some_and(|value| !value.is_empty()));
            }
        }
    }
}
