use super::*;

#[test]
fn path_reference_matches_small_hand_calculated_cases() {
    assert_eq!(solve_paths(2, 2, (9, 9), 1), (2, "RRUU".into()));
    assert_eq!(solve_paths(2, 2, (9, 9), 2), (2, "RUUR".into()));
    assert_eq!(solve_paths(2, 2, (9, 9), 3), (2, "RURU".into()));
    assert_eq!(solve_paths(2, 2, (1, 1), 3), (0, String::new()));
}

#[test]
fn xor_reference_keeps_all_minimum_weight_assignments() {
    let (penalty, answers) = solve_xor(&[(0, 1, 0, 2), (0, 1, 1, 7)]);
    assert_eq!(penalty, 2);
    assert_eq!(answers.len(), 32);
    assert_eq!(answers.first().unwrap(), "010000");
    assert_eq!(answers.last().unwrap(), "101111");
}

#[test]
fn families_are_balanced_bounded_and_shareable() {
    assert!(generate_batch(0).is_err());
    assert!(generate_batch(9).is_err());
    let batch = generate_batch(8).unwrap();
    assert_eq!(
        batch
            .iter()
            .filter(|sample| sample.family == "paths")
            .count(),
        4
    );
    assert_eq!(
        batch
            .iter()
            .map(|sample| &sample.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        8
    );
    for sample in &batch {
        assert_eq!(
            sample.evaluate_answer(&sample.expected.to_string()).verdict,
            Verdict::Correct
        );
        assert!(
            !serde_json::to_string(&sample.evaluate_answer("synthetic-private-response"))
                .unwrap()
                .contains("synthetic-private-response")
        );
    }
}

#[test]
fn unknown_transport_and_invalid_answers_never_become_incorrect() {
    let sample = challenge_from_seed([3; 32], true);
    for text in [
        "timeout",
        "{}",
        "{\"count\":\"3\",\"first\":\"RU\"}",
        "```json\n{}\n```",
        "{} {}",
    ] {
        assert_eq!(sample.evaluate_answer(text).verdict, Verdict::Inconclusive);
    }
    for response in [
        json!({"status":"incomplete"}),
        json!({"status":"failed"}),
        json!({"status":"completed","output":[{"type":"function_call"}]}),
        json!({"status":"completed","output":[{"type":"message","content":[{"type":"refusal","refusal":"No"}]}]}),
    ] {
        assert_eq!(
            sample.evaluate_response(&response).verdict,
            Verdict::Inconclusive
        );
    }
    let wrong = json!({"count":16384,"first":""});
    assert_eq!(
        sample.evaluate_answer(&wrong.to_string()).verdict,
        Verdict::Incorrect
    );
}
