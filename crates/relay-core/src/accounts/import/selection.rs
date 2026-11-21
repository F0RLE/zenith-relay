use super::MAX_IMPORT_ITEMS;
use std::collections::HashSet;

pub fn normalize_selected_item_ids(item_ids: Vec<String>) -> Result<Vec<String>, &'static str> {
    if item_ids.len() > MAX_IMPORT_ITEMS {
        return Err("selected import item count exceeds the supported limit");
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for item_id in item_ids {
        let item_id = item_id.trim();
        let Some(suffix) = item_id.strip_prefix("import_") else {
            return Err("selected import item id is invalid");
        };
        if suffix.len() != 16 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("selected import item id is invalid");
        }
        if seen.insert(item_id.to_string()) {
            normalized.push(item_id.to_string());
        }
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_preserves_order_and_deduplicates_valid_ids() {
        let selected = normalize_selected_item_ids(vec![
            " import_0123456789abcdef ".into(),
            "import_0123456789abcdef".into(),
            "import_fedcba9876543210".into(),
        ])
        .unwrap();
        assert_eq!(
            selected,
            ["import_0123456789abcdef", "import_fedcba9876543210"]
        );
    }

    #[test]
    fn selection_rejects_paths_and_excessive_raw_input() {
        assert!(normalize_selected_item_ids(vec!["../credentials".into()]).is_err());
        assert!(
            normalize_selected_item_ids(vec![
                "import_0123456789abcdef".into();
                MAX_IMPORT_ITEMS + 1
            ])
            .is_err()
        );
    }
}
