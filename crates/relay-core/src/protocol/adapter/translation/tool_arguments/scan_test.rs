use super::{integer_literal, normalize};
use std::{borrow::Cow, hint::black_box, time::Instant};

// Independent byte-by-byte scanner retained as the pre-optimization reference.
fn scalar_normalize(arguments: &str, complete: bool) -> Cow<'_, str> {
    let bytes = arguments.as_bytes();
    let mut output = None::<String>;
    let mut copied = 0;
    let mut end = arguments.len();
    let mut index = 0;
    let mut quoted = false;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if quoted => index = (index + 2).min(bytes.len()),
            b'"' => {
                quoted = !quoted;
                index += 1;
            }
            b'-' | b'0'..=b'9' if !quoted => {
                let start = index;
                index += 1;
                while index < bytes.len()
                    && matches!(bytes[index], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    index += 1;
                }
                if index == bytes.len() && !complete {
                    end = start;
                    break;
                }
                if let Some(integer) = integer_literal(&arguments[start..index]) {
                    let output =
                        output.get_or_insert_with(|| String::with_capacity(arguments.len()));
                    output.push_str(&arguments[copied..start]);
                    output.push_str(&integer);
                    copied = index;
                }
            }
            _ => index += 1,
        }
    }
    match output {
        Some(mut output) => {
            output.push_str(&arguments[copied..end]);
            Cow::Owned(output)
        }
        None => Cow::Borrowed(&arguments[..end]),
    }
}

fn assert_same(input: &str, complete: bool) {
    let expected = scalar_normalize(input, complete);
    let actual = normalize(input, complete);
    assert_eq!(actual, expected, "input={input:?}, complete={complete}");
    assert_eq!(
        matches!(actual, Cow::Borrowed(_)),
        matches!(expected, Cow::Borrowed(_)),
        "borrowed prefix differs for input={input:?}, complete={complete}"
    );
}

#[test]
fn quoted_scan_matches_scalar_for_all_unicode_prefixes_and_alignments() {
    for input in [
        r#"{"count":1000.0,"payload":"Привет 🦀 1e3","tail":12.50e2}"#,
        r#"{"s":"1.0 \"2e3\" \\ 9.0","n":-0.0,"m":9007199254740993.0}"#,
        r#"{"s":"\\\" 1.0","n":1e3,"tail":"unterminated \\"#,
        r#"{"count":1.0,"nested":["x",["2.0",3.0]],"tail":12e"#,
        r#"{"s":"\u0031\n\r\t1.0","tail":18446744073709551616.0}"#,
        "\"\\",
        "\"\\🦀\" 1.0",
        "\"raw\ncontrol\u{0000}1.0\" 2e3",
    ] {
        for alignment in 0..32 {
            let storage = format!("{}{}", " ".repeat(alignment), input);
            let input = &storage[alignment..];
            for end in 0..=input.len() {
                if input.is_char_boundary(end) {
                    for complete in [false, true] {
                        assert_same(&input[..end], complete);
                    }
                }
            }
        }
    }
}

#[test]
fn quoted_scan_matches_scalar_at_vector_boundaries() {
    for length in [0, 1, 15, 16, 31, 32, 63, 64, 127, 128, 4096] {
        for tail in ["\"}", "\\\"1.0\"}", "\\", "", "\",\"n\":1.0}"] {
            let input = format!("{{\"n\":1.0,\"s\":\"{}{tail}", "x".repeat(length));
            for complete in [false, true] {
                assert_same(&input, complete);
            }
        }
    }
}

#[test]
fn quoted_scan_matches_scalar_for_exhaustive_short_escape_number_sequences() {
    let alphabet = b"\"\\1e.,";
    for mut value in 0..alphabet.len().pow(6) {
        let mut bytes = [0; 6];
        for byte in &mut bytes {
            *byte = alphabet[value % alphabet.len()];
            value /= alphabet.len();
        }
        let input = std::str::from_utf8(&bytes).unwrap();
        for complete in [false, true] {
            assert_same(input, complete);
        }
    }
}

#[test]
#[ignore = "manual synthetic scalar versus optimized argument scanning measurement"]
fn measure_argument_normalization_scan() {
    let workloads = [
        ("short", r#"{"s":"hello","n":1}"#.to_owned()),
        ("numbers", format!("[{}0]", "1,2,3,4,5,".repeat(128))),
        (
            "text_64k",
            format!("{{\"s\":\"{}\",\"n\":1}}", "x".repeat(64 * 1024)),
        ),
        (
            "text_1m",
            format!("{{\"s\":\"{}\",\"n\":1}}", "x".repeat(1024 * 1024)),
        ),
        (
            "normalized_text_1m",
            format!("{{\"n\":1000.0,\"s\":\"{}\"}}", "x".repeat(1024 * 1024)),
        ),
        (
            "escaped_text_64k",
            format!(
                "{{\"s\":\"{}\",\"n\":1}}",
                r#"text \\ \" 1.0 Привет 🦀 "#.repeat(2048)
            ),
        ),
    ];
    for (name, input) in workloads {
        assert_same(&input, false);
        let iterations = if input.len() < 8192 { 1000 } else { 32 };
        let mut samples = [Vec::new(), Vec::new()];
        for trial in 0..7 {
            for variant in [trial % 2, 1 - trial % 2] {
                let started = Instant::now();
                for _ in 0..iterations {
                    black_box(if variant == 0 {
                        normalize(black_box(&input), false)
                    } else {
                        scalar_normalize(black_box(&input), false)
                    });
                }
                samples[variant].push(started.elapsed().as_nanos() / iterations);
            }
        }
        for (variant, mut samples) in samples.into_iter().enumerate() {
            samples.sort_unstable();
            let variant = if variant == 0 { "current" } else { "scalar" };
            eprintln!(
                "argument scan variant={variant} workload={name} bytes={} trials=7 median_ns={} min_ns={} max_ns={}",
                input.len(),
                samples[3],
                samples[0],
                samples[6]
            );
        }
    }
}
