use super::evidence;
use serde_json::json;

#[test]
fn unknown_usage_is_not_free_and_cached_tokens_are_a_subset() {
    assert_eq!(evidence(&json!({})), [None; 3]);
    assert_eq!(
        evidence(&json!({"usage":{"input_tokens":0}})),
        [Some(0), None, None]
    );
    assert_eq!(
        evidence(
            &json!({"usage":{"input_tokens":8,"input_tokens_details":{"cached_tokens":4},
        "output_tokens":2,"total_tokens":10}})
        ),
        [Some(8), Some(4), Some(2)]
    );
    for usage in [
        json!({"input_tokens":1,"input_tokens_details":{"cached_tokens":2}}),
        json!({"input_tokens":8,"output_tokens":2,"total_tokens":9}),
    ] {
        assert_eq!(evidence(&json!({"usage":usage})), [None; 3]);
    }
}
