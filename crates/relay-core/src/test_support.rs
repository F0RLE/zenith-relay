use ring::rand::{SecureRandom, SystemRandom};
use std::path::PathBuf;

mod fixture_root;

pub(crate) fn temp_root(label: &str) -> PathBuf {
    let mut random = [0u8; 16];
    SystemRandom::new().fill(&mut random).unwrap();
    fixture_root::create(label, &hex::encode(random))
}
