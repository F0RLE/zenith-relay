use std::{
    fs,
    net::TcpListener,
    path::PathBuf,
    process::{Command, Stdio},
};
use tempfile::TempDir;
use uuid::Uuid;

pub fn port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub struct Cluster {
    pub temp: TempDir,
    bin: PathBuf,
    pub pg_port: u16,
    pub password: String,
}

impl Cluster {
    pub fn new() -> Self {
        let bin = std::env::var_os("RELAY_TEST_PG_BIN").expect("set RELAY_TEST_PG_BIN");
        let c = Self::isolated(bin.into(), Uuid::new_v4().simple().to_string());
        let pass = c.temp.path().join("init-secret");
        fs::write(&pass, &c.password).unwrap();
        let init = Command::new(c.bin.join("initdb"))
            .arg("-D")
            .arg(c.data())
            .args([
                "-U",
                "fixture_boot",
                "-A",
                "scram-sha-256",
                "--no-locale",
                "-E",
                "UTF8",
                "--no-sync",
                "--pwfile",
            ])
            .arg(pass)
            .output()
            .expect("initdb");
        assert!(
            init.status.success(),
            "initdb: {}",
            String::from_utf8_lossy(&init.stderr)
        );
        c.start();
        c
    }

    fn isolated(bin: PathBuf, password: String) -> Self {
        let temp = super::fixture_files::private_root();
        Self {
            temp,
            bin,
            pg_port: port(),
            password,
        }
    }

    /// Copy only this temporary cluster, with its own WAL and a verified manifest.
    /// The copy has a different loopback port and is not started here.
    #[allow(dead_code)]
    pub fn base_backup(&self) -> Self {
        let copy = Self::isolated(self.bin.clone(), self.password.clone());
        let mut command = Command::new(self.bin.join("pg_basebackup"));
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().to_ascii_uppercase().starts_with("PG") {
                command.env_remove(key);
            }
        }
        let backup = command
            .args([
                "-h",
                "127.0.0.1",
                "-p",
                &self.pg_port.to_string(),
                "-U",
                "fixture_boot",
            ])
            .args([
                "-w",
                "-c",
                "fast",
                "-X",
                "stream",
                "--manifest-checksums=SHA256",
                "-D",
            ])
            .arg(copy.data())
            .env("PGPASSWORD", &self.password)
            .env("PGSSLMODE", "disable")
            .env("PGCONNECT_TIMEOUT", "5")
            .output()
            .expect("pg_basebackup");
        assert!(
            backup.status.success(),
            "pg_basebackup: {}",
            String::from_utf8_lossy(&backup.stderr)
        );
        let verified = Command::new(self.bin.join("pg_verifybackup"))
            .args(["--exit-on-error"])
            .arg(copy.data())
            .output()
            .expect("pg_verifybackup");
        assert!(
            verified.status.success(),
            "pg_verifybackup: {}",
            String::from_utf8_lossy(&verified.stderr)
        );
        copy
    }

    pub fn start(&self) {
        let started = self
            .control()
            .args([
                "-o",
                &format!(
                    "-h 127.0.0.1 -p {} -c unix_socket_directories=",
                    self.pg_port
                ),
                "-l",
            ])
            .arg(self.temp.path().join("pg.log"))
            .arg("start")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("pg_ctl start");
        assert!(
            started.success(),
            "isolated PostgreSQL did not start: {}",
            fs::read_to_string(self.temp.path().join("pg.log"))
                .unwrap_or_default()
                .chars()
                .take(4096)
                .flat_map(char::escape_default)
                .collect::<String>()
        );
    }

    fn data(&self) -> PathBuf {
        self.temp.path().join("pgdata")
    }

    fn control(&self) -> Command {
        let mut cmd = Command::new(self.bin.join("pg_ctl"));
        cmd.arg("-D").arg(self.data()).args(["-w", "-t", "30"]);
        cmd
    }

    pub fn dsn(&self, role: &str) -> String {
        format!(
            "postgresql://{role}:{}@127.0.0.1:{}/postgres?sslmode=disable",
            self.password, self.pg_port
        )
    }

    pub async fn create_logins(&self, pool: &sqlx::PgPool, roles: &[&str]) {
        let mut tx = pool.begin().await.unwrap();
        for &role in roles {
            sqlx::query(
                "SELECT set_config('fixture.role',$1,true), set_config('fixture.password',$2,true)",
            )
            .bind(role)
            .bind(&self.password)
            .execute(&mut *tx)
            .await
            .unwrap();
            sqlx::query(
                "DO $$ BEGIN EXECUTE format('CREATE ROLE %I LOGIN PASSWORD %L',
                 current_setting('fixture.role'), current_setting('fixture.password')); END $$",
            )
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
    }

    pub fn stop(&self) {
        assert!(
            self.control()
                .args(["-m", "fast", "stop"])
                .status()
                .unwrap()
                .success()
        );
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        if self.data().join("postmaster.pid").is_file() {
            let _ = self.control().args(["-m", "immediate", "stop"]).output();
        }
    }
}
