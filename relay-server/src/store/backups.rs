use super::sqlite::{db_error, io_error, Store};
use std::{fs, path::Path};

impl Store {
    pub fn backup_to(&self, destination: &Path) -> Result<(), String> {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        if destination.exists() {
            return Err("backup database destination already exists".to_string());
        }
        self.lock()?
            .backup(rusqlite::MAIN_DB, destination, None)
            .map_err(db_error)
    }
}
