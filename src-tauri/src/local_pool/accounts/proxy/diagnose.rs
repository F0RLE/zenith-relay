use super::ProxyPool;
use crate::storage_paths::StoragePaths;
use serde_json::json;
use std::{io::Write, path::Path};
use zenith_relay_core::{
    providers::chatgpt::probe_subscription_proxy, scheduler::refresh::http::ManagementHttpScope,
};

const FLAG: &str = "--diagnose-subscription-proxies";
const AUTHORIZATION_FLAG: &str = "--diagnose-subscription-authorization";

mod authorization;

pub(crate) fn run_if_requested() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().collect();
    let result = match diagnostic_root(&args) {
        Ok(None) => return None,
        Err(code) => Err(code),
        Ok(Some(root)) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "runtime_unavailable")
            .and_then(|runtime| {
                if args[1] == AUTHORIZATION_FLAG {
                    runtime.block_on(authorization::compare(root))
                } else {
                    runtime.block_on(compare(root))
                }
            }),
    };
    Some(write_result(result, &mut std::io::stdout().lock()))
}

fn write_result(result: Result<serde_json::Value, &'static str>, writer: &mut impl Write) -> i32 {
    let (output, exit_code) = match result {
        Ok(output) => (output, 0),
        Err(code) => (json!({"status":"error","code":code}), 2),
    };
    if serde_json::to_writer(&mut *writer, &output).is_err()
        || writer.write_all(b"\n").is_err()
        || writer.flush().is_err()
    {
        return 2;
    }
    exit_code
}

fn diagnostic_root(args: &[std::ffi::OsString]) -> Result<Option<&Path>, &'static str> {
    if !args
        .iter()
        .any(|arg| arg == FLAG || arg == AUTHORIZATION_FLAG)
    {
        return Ok(None);
    }
    if args.len() != 3
        || !matches!(args[1].to_str(), Some(FLAG | AUTHORIZATION_FLAG))
        || !Path::new(&args[2]).is_absolute()
    {
        return Err("expected_absolute_relay_data_root");
    }
    Ok(Some(Path::new(&args[2])))
}

async fn compare(root: &Path) -> Result<serde_json::Value, &'static str> {
    let metadata = std::fs::symlink_metadata(root).map_err(|_| "data_root_unavailable")?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("data_root_unsafe");
    }
    let vault_root = StoragePaths::from_root(root).vault_root();
    let pool = ProxyPool::load_read_only(&vault_root).map_err(|_| "proxy_pool_unavailable")?;
    let routes: Vec<_> = pool.refresh_scopes().take(2).collect();
    if routes.len() != 2 {
        return Err("two_saved_proxies_required");
    }
    let mut results = Vec::with_capacity(2);
    for (index, route) in routes.into_iter().enumerate() {
        let config = pool
            .config(route.id)
            .map_err(|_| "proxy_configuration_invalid")?;
        let expected = pool
            .stored_url(route.id)
            .map_err(|_| "proxy_configuration_invalid")?;
        let proxy_id = route.id.to_owned();
        let checked_root = vault_root.clone();
        let scope = ManagementHttpScope::checked(move || {
            ProxyPool::load_read_only(&checked_root)
                .and_then(|current| current.stored_url(&proxy_id))
                .is_ok_and(|current| current == expected)
        });
        let observations = probe_subscription_proxy(&config, scope)
            .await
            .map_err(|_| "proxy_client_unavailable")?;
        let deferred = observations.iter().any(|item| {
            item.http_status == Some(429) || item.retry_after_ms.is_some_and(|delay| delay > 0)
        });
        results.push(json!({"proxy":if index == 0 {"A"} else {"B"}, "observations":observations}));
        if deferred {
            break;
        }
    }
    Ok(
        json!({"status":if results.len() == 2 {"complete"} else {"deferred"}, "authentication":"none", "generationTested":false,
        "workingConfigurationChanged":false, "routes":results}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn diagnostic_output_is_one_json_line_with_the_expected_exit_code() {
        for (result, expected, exit_code) in [
            (
                Ok(json!({"status":"complete"})),
                json!({"status":"complete"}),
                0,
            ),
            (
                Err("proxy_pool_unavailable"),
                json!({"status":"error","code":"proxy_pool_unavailable"}),
                2,
            ),
        ] {
            let mut output = Vec::new();
            assert_eq!(write_result(result, &mut output), exit_code);
            assert_eq!(output.last(), Some(&b'\n'));
            assert_eq!(output.iter().filter(|&&byte| byte == b'\n').count(), 1);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&output).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn diagnostic_output_write_and_flush_failures_return_an_error_exit() {
        struct FailingWriter {
            fail_flush: bool,
        }

        impl Write for FailingWriter {
            fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                if self.fail_flush {
                    Ok(buffer.len())
                } else {
                    Err(std::io::ErrorKind::BrokenPipe.into())
                }
            }

            fn flush(&mut self) -> std::io::Result<()> {
                if self.fail_flush {
                    Err(std::io::ErrorKind::BrokenPipe.into())
                } else {
                    Ok(())
                }
            }
        }

        for fail_flush in [false, true] {
            assert_eq!(
                write_result(
                    Ok(json!({"status":"complete"})),
                    &mut FailingWriter { fail_flush }
                ),
                2
            );
        }
    }

    #[test]
    fn diagnostic_arguments_never_turn_into_a_normal_app_launch() {
        let ordinary = vec![OsString::from("relay"), OsString::from("--tray")];
        assert_eq!(diagnostic_root(&ordinary).unwrap(), None);
        for args in [
            vec![OsString::from("relay"), OsString::from(FLAG)],
            vec![
                OsString::from("relay"),
                OsString::from(FLAG),
                OsString::from("relative"),
            ],
            vec![
                OsString::from("relay"),
                OsString::from("--tray"),
                OsString::from(FLAG),
            ],
        ] {
            assert!(diagnostic_root(&args).is_err());
        }
        let root = std::env::temp_dir().join("synthetic-relay-root");
        let args = vec![
            OsString::from("relay"),
            OsString::from(FLAG),
            root.clone().into_os_string(),
        ];
        assert_eq!(diagnostic_root(&args).unwrap(), Some(root.as_path()));
        let authorization_args = vec![
            OsString::from("relay"),
            OsString::from(AUTHORIZATION_FLAG),
            root.clone().into_os_string(),
        ];
        assert_eq!(
            diagnostic_root(&authorization_args).unwrap(),
            Some(root.as_path())
        );
        assert!(diagnostic_root(&authorization_args[..2]).is_err());
    }
}
