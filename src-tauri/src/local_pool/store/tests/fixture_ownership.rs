use super::*;

#[test]
fn temporary_store_roots_are_reserved_before_any_store_operation() {
    let first = temp_root();
    let second = temp_root();
    let distinct = first != second;
    let first_reserved = fs::symlink_metadata(&first)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink());
    let second_reserved = fs::symlink_metadata(&second)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink());
    #[cfg(unix)]
    let private = {
        use std::os::unix::fs::PermissionsExt;
        [&first, &second].iter().all(|path| {
            fs::symlink_metadata(path)
                .is_ok_and(|metadata| metadata.permissions().mode() & 0o077 == 0)
        })
    };
    if first_reserved {
        fs::remove_dir_all(&first).unwrap();
    }
    if second_reserved {
        fs::remove_dir_all(&second).unwrap();
    }

    assert!(distinct);
    assert!(
        first_reserved,
        "fixture must exclusively reserve its directory"
    );
    assert!(
        second_reserved,
        "every fixture must own a distinct directory"
    );
    #[cfg(unix)]
    assert!(private, "shared-temp fixtures must be owner-private");
}
