//! Manual, capped probes against an already running local Relay. Never runs at startup.
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    error::Error,
    io,
    time::{Duration, Instant},
};
use zenith_relay_core::{
    is_valid_model_token,
    quality::{Challenge, MAX_SAMPLES, Observation, Verdict, generate_batch},
};

fn invalid(message: &'static str) -> io::Error {
    io::Error::other(message)
}

async fn read_response(response: reqwest::Response) -> Result<Value, &'static str> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "body_unavailable")?;
        if body.len().saturating_add(chunk.len()) > 1024 * 1024 {
            return Err("response_too_large");
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "response_not_json")
}

async fn observe(
    client: &reqwest::Client,
    url: &url::Url,
    key: &str,
    model: &str,
    effort: Option<&str>,
    sample: &Challenge,
) -> (Observation, Option<u16>, Option<Value>) {
    let mut body = json!({"model":model,"input":sample.prompt,"stream":false,"store":false,"tools":[],"max_output_tokens":8192});
    if let Some(effort) = effort {
        body["reasoning"] = json!({"effort":effort});
    }
    let response = match client
        .post(url.clone())
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return (sample.unavailable("request_unavailable"), None, None),
    };
    let status = response.status();
    if !status.is_success() {
        return (
            sample.unavailable("http_rejection"),
            Some(status.as_u16()),
            None,
        );
    }
    match read_response(response).await {
        Ok(body) => {
            let usage = body
                .get("usage")
                .filter(|usage| usage.is_object())
                .map(|usage| {
                    // Never echo arbitrary provider bodies through a nominal usage field.
                    let mut safe = serde_json::Map::new();
                    for field in ["input_tokens", "output_tokens", "total_tokens"] {
                        if let Some(count) = usage.get(field).and_then(Value::as_u64) {
                            safe.insert(field.into(), json!(count));
                        }
                    }
                    Value::Object(safe)
                });
            (
                sample.evaluate_response(&body),
                Some(status.as_u16()),
                usage,
            )
        }
        Err(reason) => (sample.unavailable(reason), Some(status.as_u16()), None),
    }
}

struct ProbeConfig {
    url: url::Url,
    models: Vec<String>,
    samples: usize,
    effort: Option<String>,
}

fn configuration(args: &[String]) -> Result<ProbeConfig, io::Error> {
    if args.len() < 4 || args.len() > 5 || args[0] != "--allow-requests" {
        return Err(invalid(
            "usage: quality_probe --allow-requests http://127.0.0.1:PORT/v1 model-a,model-b samples[1..8] [effort]; set ZENITH_RELAY_PROBE_KEY in the environment",
        ));
    }
    let mut url = url::Url::parse(&args[1]).map_err(|_| invalid("invalid local Relay URL"))?;
    let loopback = matches!(url.host(),Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        || matches!(url.host(),Some(url::Host::Ipv6(ip)) if ip.is_loopback());
    if !loopback
        || url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "/v1" | "/v1/")
    {
        return Err(invalid(
            "probe URL must be an explicit loopback HTTP /v1 endpoint without credentials, query, or fragment",
        ));
    }
    url.set_path("/v1/responses");
    let models: Vec<&str> = args[2].split(',').collect();
    if models.is_empty()
        || models.len() > 4
        || models.iter().any(|model| !is_valid_model_token(model))
        || models
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != models.len()
    {
        return Err(invalid("provide one to four distinct model IDs"));
    }
    let samples = args[3]
        .parse::<usize>()
        .map_err(|_| invalid("invalid samples"))?;
    if !(1..=MAX_SAMPLES).contains(&samples) {
        return Err(invalid("samples must be between 1 and 8"));
    }
    let effort = args.get(4).map(String::as_str);
    if effort.is_some_and(|effort| {
        !matches!(
            effort,
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
        )
    }) {
        return Err(invalid("invalid effort"));
    }
    Ok(ProbeConfig {
        url,
        models: models.into_iter().map(str::to_owned).collect(),
        samples,
        effort: effort.map(str::to_owned),
    })
}

fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(180))
        .build()
}

fn write_report(output: &mut impl io::Write, report: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, report).map_err(io::Error::other)?;
    output.write_all(b"\n")?;
    output.flush()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = configuration(&std::env::args().skip(1).collect::<Vec<_>>())?;
    let ProbeConfig {
        url,
        models,
        samples,
        effort,
    } = config;
    let batch = generate_batch(samples).map_err(invalid)?;
    let key = std::env::var("ZENITH_RELAY_PROBE_KEY")
        .map_err(|_| invalid("ZENITH_RELAY_PROBE_KEY is required"))?;
    if key.trim().is_empty() {
        return Err(invalid("ZENITH_RELAY_PROBE_KEY is empty").into());
    }
    let client = client()?;
    let stdout = io::stdout();
    let mut output = io::BufWriter::new(stdout.lock());
    // Sequential, once per sample/model, max 32 generations; no retries or judge
    // requests. Report observations only, never disable or reconfigure a route.
    let mut counts = vec![[0_u32; 3]; models.len()];
    for sample in &batch {
        for (index, model) in models.iter().enumerate() {
            let started = Instant::now();
            let (result, status, usage) =
                observe(&client, &url, &key, model, effort.as_deref(), sample).await;
            counts[index][match result.verdict {
                Verdict::Correct => 0,
                Verdict::Incorrect => 1,
                Verdict::Inconclusive => 2,
            }] += 1;
            write_report(
                &mut output,
                &json!({"model":model,"client_protocol":"responses","requested_effort":effort,"observation":result,"http_status":status,"usage":usage,"latency_ms":started.elapsed().as_millis() as u64}),
            )?;
        }
    }
    for (model, counts) in models.iter().zip(counts) {
        write_report(
            &mut output,
            &json!({"model":model,"samples":samples,"correct":counts[0],"incorrect":counts[1],"inconclusive":counts[2],"action":"observe_only"}),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_are_independent_json_lines() {
        let mut output = Vec::new();
        let reports = [
            json!({"model":"synthetic","correct":1}),
            json!({"action":"observe_only"}),
        ];
        for report in &reports {
            write_report(&mut output, report).unwrap();
        }
        let text = String::from_utf8(output).unwrap();
        let decoded = text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(decoded, reports);
        assert!(text.ends_with('\n'));
    }

    fn args(url: &str, models: &str, samples: &str) -> Vec<String> {
        ["--allow-requests", url, models, samples]
            .map(str::to_owned)
            .to_vec()
    }

    #[test]
    fn explicit_consent_loopback_and_request_limits_are_required() {
        assert!(configuration(&[]).is_err());
        for url in [
            "https://127.0.0.1/v1",
            "http://example.invalid/v1",
            "http://localhost/v1",
            "http://user:private@127.0.0.1/v1",
            "http://127.0.0.1/v1?key=private",
            "http://127.0.0.1/v1#private",
            "http://127.0.0.1/v1/v2",
        ] {
            let error = configuration(&args(url, "test", "1"))
                .err()
                .unwrap()
                .to_string();
            assert!(!error.contains("private"));
        }
        for models in ["", "one,one", "a,b,c,d,e"] {
            assert!(configuration(&args("http://127.0.0.1/v1", models, "1")).is_err());
        }
        for samples in ["0", "9", "-1"] {
            assert!(configuration(&args("http://127.0.0.1/v1", "test", samples)).is_err());
        }
        let parsed = configuration(&args("http://[::1]:8080/v1/", "a,b,c,d", "8")).unwrap();
        assert_eq!(parsed.models.len() * parsed.samples, 32);
        assert_eq!(parsed.url.path(), "/v1/responses");
        assert!(parsed.effort.is_none());
    }

    #[tokio::test]
    async fn rejection_is_not_retried_and_provider_text_is_not_reported() {
        use axum::{Router, http::StatusCode, routing::post};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let app = Router::new().route(
            "/v1/responses",
            post(move || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::TOO_MANY_REQUESTS, "synthetic-private-body")
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = url::Url::parse(&format!(
            "http://{}/v1/responses",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let sample = generate_batch(1).unwrap().remove(0);
        let (observation, status, usage) = observe(
            &client().unwrap(),
            &url,
            "synthetic-key",
            "test",
            None,
            &sample,
        )
        .await;
        assert_eq!(status, Some(429));
        assert_eq!(observation.verdict, Verdict::Inconclusive);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(usage.is_none());
        let diagnostic = serde_json::to_string(&observation).unwrap();
        assert!(!diagnostic.contains("synthetic-key"));
        assert!(!diagnostic.contains("synthetic-private-body"));
        server.abort();
    }
}
