use std::process::Command;

const PROXY_CHILD: &str = "ZENITH_RELAY_TEST_SYSTEM_PROXY_CHILD";

/// Returns true after verifying the test in a child with inherited proxies enabled.
pub(crate) async fn run_isolated_proxy_test(test_path: &str) -> bool {
    if std::env::var_os(PROXY_CHILD).is_some() {
        return false;
    }
    let proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    let test_name = test_path.split_once("::").unwrap().1;
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test_name, "--nocapture"])
        .env(PROXY_CHILD, "1");
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env(key, &proxy_url);
    }
    for key in ["NO_PROXY", "no_proxy"] {
        command.env(key, "");
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = tokio::task::spawn_blocking(move || command.output())
        .await
        .unwrap()
        .unwrap();
    drop(proxy);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("running 1 test"),
        "child filter did not run one test: {stdout}{stderr}"
    );
    assert!(
        output.status.success(),
        "isolated proxy test failed: {stdout}{stderr}"
    );
    true
}
