use std::{ffi::OsString, io::Write};
use zenith_relay_core::Http2RetryDiagnostic;

const FLAG: &str = "--diagnose-http2-retries";

pub(crate) fn run_if_requested() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().collect();
    let result = match requested(&args) {
        Ok(false) => return None,
        Err(code) => Err(code),
        Ok(true) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "http2_diagnostic_runtime_unavailable")
            .and_then(|runtime| runtime.block_on(zenith_relay_core::diagnose_http2_retries())),
    };
    Some(write_result(result, &mut std::io::stdout().lock()))
}

fn requested(args: &[OsString]) -> Result<bool, &'static str> {
    if !args
        .iter()
        .skip(1)
        .any(|arg| arg.to_string_lossy().to_ascii_lowercase().starts_with(FLAG))
    {
        return Ok(false);
    }
    if args.len() != 2 || args[1] != FLAG {
        return Err("http2_diagnostic_unexpected_arguments");
    }
    Ok(true)
}

fn write_result(
    result: Result<Http2RetryDiagnostic, &'static str>,
    writer: &mut impl Write,
) -> i32 {
    let (output, exit_code) = match result {
        Ok(report) => {
            let exit_code = if report.passed { 0 } else { 2 };
            (
                serde_json::json!({
                    "status": if report.passed { "complete" } else { "failed" },
                    "scope": "loopback_http2_prior_knowledge",
                    "liveProviderRequests": false,
                    "configurationChanged": false,
                    "observations": report,
                }),
                exit_code,
            )
        }
        Err(code) => (serde_json::json!({"status":"error","code":code}), 2),
    };
    super::cli_output::write_report(&output, exit_code, writer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_argument_errors_cannot_launch_the_desktop_or_open_account_storage() {
        let args = |items: &[&str]| items.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(!requested(&args(&["relay", "--tray"])).unwrap());
        assert!(requested(&args(&["relay", FLAG])).unwrap());
        for items in [
            vec!["relay", FLAG, "extra"],
            vec!["relay", "--tray", FLAG],
            vec!["relay", "--diagnose-http2-retries=value"],
            vec!["relay", "--DIAGNOSE-HTTP2-RETRIES"],
            vec!["relay", FLAG, "--diagnose-subscription-proxies"],
        ] {
            assert!(requested(&args(&items)).is_err());
        }
    }

    #[test]
    fn diagnostic_output_is_one_safe_json_line_and_failures_have_a_nonzero_exit() {
        for passed in [false, true] {
            let report = Http2RetryDiagnostic {
                passed,
                default_client_requests: 3,
                cases: Vec::new(),
            };
            let mut output = Vec::new();
            assert_eq!(
                write_result(Ok(report), &mut output),
                if passed { 0 } else { 2 }
            );
            assert_eq!(output.iter().filter(|&&byte| byte == b'\n').count(), 1);
            let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(value["status"], if passed { "complete" } else { "failed" });
            assert_eq!(value["liveProviderRequests"], false);
            assert_eq!(value["configurationChanged"], false);
        }
        let mut output = Vec::new();
        assert_eq!(
            write_result(Err("http2_diagnostic_timeout"), &mut output),
            2
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output).unwrap(),
            serde_json::json!({"status":"error","code":"http2_diagnostic_timeout"})
        );
    }
}
