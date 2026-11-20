mod cluster;
pub use cluster::{Cluster, port};
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

pub struct Process(pub Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Cluster {
    pub fn role_file(&self, role: &str) -> std::path::PathBuf {
        let secret = self.temp.path().join(role);
        fs::write(&secret, self.dsn(role)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
        }
        secret
    }
    pub fn cli(&self, role: &str, cfg: &Path) -> Command {
        let secret = self.role_file(role);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_zenith-relay-hosted"));
        cmd.env("ZENITH_RELAY_HOSTED_CONFIG", cfg)
            .env("ZENITH_RELAY_HOSTED_DATABASE_URL_FILE", secret)
            .stdin(Stdio::null());
        cmd
    }
}

pub async fn ready(child: &mut Process, client: &reqwest::Client, url: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            assert!(child.0.try_wait().unwrap().is_none(), "serve exited early");
            if let Ok(r) = client.get(url).send().await
                && r.status().is_success()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("serve readiness");
}
