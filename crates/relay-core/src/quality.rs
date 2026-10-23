//! Observation-only reasoning probes with local, bounded reference solvers.
//! This module has no scheduler, credentials, networking, or policy mutation.

use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const GENERATOR_VERSION: u8 = 1;
pub const MAX_SAMPLES: usize = 8;

pub struct Challenge {
    pub id: String,
    pub family: &'static str,
    pub prompt: String,
    expected: Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Correct,
    Incorrect,
    Inconclusive,
}

#[derive(Debug, Serialize)]
pub struct Observation {
    pub question_id: String,
    pub family: &'static str,
    pub generator_version: u8,
    pub verdict: Verdict,
    pub reason: &'static str,
}

/// Generate a batch once and share it across compared routes. No fixed public
/// question, model-generated reference, or answer from the tested route is used.
pub fn generate_batch(samples: usize) -> Result<Vec<Challenge>, &'static str> {
    if !(1..=MAX_SAMPLES).contains(&samples) {
        return Err("samples must be between 1 and 8");
    }
    let random = SystemRandom::new();
    let mut challenges = Vec::with_capacity(samples);
    let mut seen = std::collections::HashSet::new();
    for index in 0..samples {
        let mut unique = None;
        for _ in 0..32 {
            let mut seed = [0_u8; 32];
            random
                .fill(&mut seed)
                .map_err(|_| "quality probe entropy unavailable")?;
            let challenge = challenge_from_seed(seed, index % 2 == 0);
            if seen.insert(challenge.id.clone()) {
                unique = Some(challenge);
                break;
            }
        }
        challenges.push(unique.ok_or("quality probe generator exhausted")?);
    }
    Ok(challenges)
}

fn challenge_from_seed(seed: [u8; 32], paths: bool) -> Challenge {
    let (family, prompt, expected) = if paths {
        let width = 4 + usize::from(seed[0] % 4);
        let height = 4 + usize::from(seed[1] % 4);
        let blocked = (
            1 + usize::from(seed[2]) % (width - 1),
            1 + usize::from(seed[3]) % (height - 1),
        );
        let turns = 2 + usize::from(seed[4]) % 6;
        let (count, first) = solve_paths(width, height, blocked, turns);
        ("paths", format!("On an integer grid travel from (0,0) to ({width},{height}) using R=(1,0) or U=(0,1). Never visit ({},{}). Use exactly {turns} turns; a turn means two adjacent different letters, and the first step is not a turn. Count all valid paths and give the lexicographically smallest path with R<U, or an empty string if none. Return only JSON {{\"count\":integer,\"first\":string}}, no explanation or code fences.",blocked.0,blocked.1), json!({"count":count,"first":first}))
    } else {
        let mut rows = Vec::new();
        for index in 0..10 {
            let a = usize::from(seed[index]) % 6;
            let b = (a + 1 + usize::from(seed[index + 10]) % 5) % 6;
            rows.push((
                a,
                b,
                seed[(index + 20) % 32] % 2,
                1 + u32::from(seed[(index + 17) % 32] % 9),
            ));
        }
        let (penalty, assignments) = solve_xor(&rows);
        let mut prompt = "Bits A,B,C,D,E,F are each 0 or 1. The following observations may be false. Minimize the SUM of penalties for false observations, not the number of false observations.\n".to_owned();
        for &(a, b, value, weight) in &rows {
            prompt.push_str(&format!(
                "{} XOR {} = {value}; penalty {weight}\n",
                (b'A' + a as u8) as char,
                (b'A' + b as u8) as char
            ));
        }
        prompt.push_str("Return only JSON {\"penalty\":integer,\"assignments\":[all optimal six-bit strings in ABCDEF order, sorted lexicographically]}, no explanation or code fences.");
        (
            "weighted_xor",
            prompt,
            json!({"penalty":penalty,"assignments":assignments}),
        )
    };
    let id = hex::encode(Sha256::digest(prompt.as_bytes()));
    Challenge {
        id,
        family,
        prompt,
        expected,
    }
}

fn solve_paths(
    width: usize,
    height: usize,
    blocked: (usize, usize),
    turns: usize,
) -> (u64, String) {
    // At most 14 moves. Enumerating at most 2^14 words is bounded and independent
    // of a provider. Bit order gives R<U, so the first solution is canonical.
    let steps = width + height;
    let mut count = 0;
    let mut first = String::new();
    for bits in 0..(1_u32 << steps) {
        let (mut x, mut y, mut changes, mut previous) = (0, 0, 0, None);
        let mut path = String::with_capacity(steps);
        let mut valid = true;
        for index in (0..steps).rev() {
            let up = bits & (1 << index) != 0;
            changes += usize::from(previous.is_some_and(|before| before != up));
            previous = Some(up);
            if up {
                y += 1;
                path.push('U');
            } else {
                x += 1;
                path.push('R');
            }
            if x > width || y > height || (x, y) == blocked || changes > turns {
                valid = false;
                break;
            }
        }
        if valid && x == width && y == height && changes == turns {
            count += 1;
            if first.is_empty() {
                first = path;
            }
        }
    }
    (count, first)
}

fn solve_xor(rows: &[(usize, usize, u8, u32)]) -> (u32, Vec<String>) {
    let mut minimum = u32::MAX;
    let mut answers = Vec::new();
    for bits in 0..64_u8 {
        let penalty: u32 = rows
            .iter()
            .filter(|&&(a, b, value, _)| (((bits >> (5 - a)) ^ (bits >> (5 - b))) & 1) != value)
            .map(|row| row.3)
            .sum();
        if penalty < minimum {
            minimum = penalty;
            answers.clear();
        }
        if penalty == minimum {
            answers.push(format!("{bits:06b}"));
        }
    }
    (minimum, answers)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathsAnswer {
    count: u64,
    first: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct XorAnswer {
    penalty: u32,
    assignments: Vec<String>,
}

impl Challenge {
    fn observation(&self, verdict: Verdict, reason: &'static str) -> Observation {
        Observation {
            question_id: self.id.clone(),
            family: self.family,
            generator_version: GENERATOR_VERSION,
            verdict,
            reason,
        }
    }

    pub fn unavailable(&self, reason: &'static str) -> Observation {
        self.observation(Verdict::Inconclusive, reason)
    }

    pub fn evaluate_answer(&self, text: &str) -> Observation {
        if text.len() > 64 * 1024 {
            return self.unavailable("answer_too_large");
        }
        let Ok(answer) = serde_json::from_str::<Value>(text) else {
            return self.unavailable("answer_not_json");
        };
        let well_formed = if self.family == "paths" {
            serde_json::from_value::<PathsAnswer>(answer.clone()).is_ok_and(|answer| {
                answer.first.len() <= 14
                    && answer.first.bytes().all(|b| matches!(b, b'R' | b'U'))
                    && answer.count <= (1 << 14)
            })
        } else {
            serde_json::from_value::<XorAnswer>(answer.clone()).is_ok_and(|answer| {
                answer.penalty <= 90
                    && answer.assignments.len() <= 64
                    && answer.assignments.iter().all(|bits| {
                        bits.len() == 6 && bits.bytes().all(|b| matches!(b, b'0' | b'1'))
                    })
            })
        };
        if !well_formed {
            return self.unavailable("answer_shape_invalid");
        }
        self.observation(
            if answer == self.expected {
                Verdict::Correct
            } else {
                Verdict::Incorrect
            },
            "local_reference_comparison",
        )
    }

    /// Transport errors, refusals, tool calls and unfinished generations are
    /// observations of availability, not evidence of lower reasoning quality.
    pub fn evaluate_response(&self, response: &Value) -> Observation {
        if response["status"] != "completed"
            || response.get("error").is_some_and(|error| !error.is_null())
        {
            return self.unavailable("response_not_completed");
        }
        let Some(items) = response["output"].as_array() else {
            return self.unavailable("response_shape_invalid");
        };
        let mut text = String::new();
        for item in items {
            if item["type"] == "reasoning" {
                continue;
            }
            if item["type"] != "message" {
                return self.unavailable("unexpected_output_item");
            }
            let Some(parts) = item["content"].as_array() else {
                return self.unavailable("response_shape_invalid");
            };
            for part in parts {
                if part["type"] != "output_text" {
                    return self.unavailable("refusal_or_nontext_output");
                }
                let Some(fragment) = part["text"].as_str() else {
                    return self.unavailable("response_shape_invalid");
                };
                if text.len().saturating_add(fragment.len()) > 64 * 1024 {
                    return self.unavailable("answer_too_large");
                }
                text.push_str(fragment);
            }
        }
        self.evaluate_answer(&text)
    }
}

#[cfg(test)]
mod tests;
