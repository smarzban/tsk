//! User-private file modes for on-disk state.
//!
//! State files hold the user's tasks; they are nobody else's business. On Unix
//! the state directories are `0700` and the files `0600` (`tsk.json`,
//! `trash.jsonl`, backups, the lock file, temp files, `update.json`), both
//! for fresh creation and tightened after the fact for paths an older version
//! or a looser umask left readable. Tightening only ever strips bits: an
//! existing stricter mode (a file an admin locked to `0400`, a read-only
//! directory) is left as it is. No permission is claimed on other platforms:
//! there these helpers compile down to plain create/copy.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(test)]
thread_local! {
    static BEFORE_LOCK_OPEN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn run_before_lock_open_hook() {
    if let Some(hook) = BEFORE_LOCK_OPEN.with(|slot| slot.borrow_mut().take()) {
        hook();
    }
}

#[cfg(not(test))]
fn run_before_lock_open_hook() {}

/// Create `path` (recursively) as a user-private directory (`0700` on Unix),
/// stripping any group/other access from one that already exists. Stricter
/// existing modes are kept.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    reject_reparse_ancestors(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_reparse_or_symlink(&metadata) || !metadata.is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "state directory must be a real directory, not a symlink",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir_all(path)?,
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "state directory must be a real directory, not a symlink",
        ));
    }
    #[cfg(unix)]
    tighten_private_dir(path)?;
    Ok(())
}

#[cfg(unix)]
fn tighten_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let path_metadata = fs::symlink_metadata(path)?;
    if !path_metadata.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "state directory must be a directory, not a symlink",
        ));
    }

    // Apply chmod through a verified open handle. This prevents a final-path
    // symlink, or a directory swapped between metadata and open, from redirecting
    // the permission change to another target.
    let directory = File::open(path)?;
    let opened_metadata = directory.metadata()?;
    if path_metadata.dev() != opened_metadata.dev() || path_metadata.ino() != opened_metadata.ino()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "state directory changed while permissions were checked",
        ));
    }
    let mode = opened_metadata.permissions().mode();
    directory.set_permissions(fs::Permissions::from_mode(mode & 0o700))
}

/// Reject any existing Windows path component that can redirect later filesystem operations.
#[cfg(windows)]
pub(crate) fn reject_reparse_ancestors(path: &Path) -> io::Result<()> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    for ancestor in absolute.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if is_reparse_or_symlink(&metadata) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "path contains a reparse-point ancestor: {}",
                        ancestor.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT also covers junctions, which are not reported by
        // FileType::is_symlink but must not redirect state or setup writes.
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        return metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[cfg(not(windows))]
    false
}

/// Atomically publish `from` at `to`. Windows requests write-through replacement so a
/// successful save includes the directory-entry update, matching the durability boundary Unix
/// gets from the following directory sync.
pub(crate) fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };

        let from = from
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let to = to
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: both buffers are stable, NUL-terminated UTF-16 strings for the duration of
        // the call. MoveFileExW does not retain either pointer.
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        return Ok(());
    }
    #[cfg(not(windows))]
    fs::rename(from, to)
}

/// Create a file holding owner-only content (`0600` on Unix when created),
/// truncating an existing one. Temp files and their renamed targets go through
/// here so a document is never briefly world-readable.
pub fn create_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Open (or create) a lock file without truncating it, `0600` on Unix when
/// created. Existing files are tightened by the caller after open.
pub fn open_lock_file(path: &Path) -> io::Result<File> {
    // An absent entry is created atomically. If another process wins that race, retry
    // through the existing-file path, which never creates through a newly planted link.
    for _ in 0..3 {
        let before = match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() && !is_reparse_or_symlink(&metadata) => {
                Some(metadata)
            }
            Ok(_) => return invalid_lock_path(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        run_before_lock_open_hook();
        let result = match before.as_ref() {
            Some(_) => lock_open_options(false).open(path),
            None => lock_open_options(true).open(path),
        };
        let file = match result {
            Ok(file) => file,
            Err(error)
                if (before.is_none() && error.kind() == io::ErrorKind::AlreadyExists)
                    || (before.is_some() && error.kind() == io::ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let opened = file.metadata()?;
        let current = match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() && !is_reparse_or_symlink(&metadata) => {
                metadata
            }
            Ok(_) => return invalid_lock_path(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if opened.file_type().is_file()
            && same_file_identity(&opened, &current)
            && before
                .as_ref()
                .is_none_or(|before| same_file_identity(before, &opened))
        {
            return Ok(file);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "lock path changed repeatedly while opening",
    ))
}

fn lock_open_options(create_new: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(create_new)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn invalid_lock_path<T>() -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "lock path must be a regular file, not a symlink",
    ))
}

#[cfg(unix)]
fn same_file_identity(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file_identity(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.file_type().is_file() && b.file_type().is_file()
}

/// Tighten an existing file to owner-only (`0600`) on Unix by stripping
/// group/other bits, never granting owner bits the file did not have. Missing
/// paths and other platforms are no-ops; only regular files are touched, never
/// symlinks. Best effort: a failed tighten must not fail the surrounding
/// operation.
pub fn tighten_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let Ok(path_metadata) = fs::symlink_metadata(path) else {
            return;
        };
        if !path_metadata.file_type().is_file() {
            return;
        }
        let Ok(file) = File::open(path) else {
            return;
        };
        let Ok(opened_metadata) = file.metadata() else {
            return;
        };
        if path_metadata.dev() != opened_metadata.dev()
            || path_metadata.ino() != opened_metadata.ino()
        {
            return;
        }
        let mode = opened_metadata.permissions().mode();
        let _ = file.set_permissions(fs::Permissions::from_mode(mode & 0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    struct TempDirGuard(std::path::PathBuf);

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::set_permissions(self.0.join("target"), fs::Permissions::from_mode(0o700));
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn open_lock_file_does_not_follow_an_absent_path_symlink_race() {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("tsk-fsperm-lock-race-{}-{seq}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir root");
        let _guard = TempDirGuard(root.clone());
        let lock = root.join("lock");
        let target = root.join("outside-target");
        let hook_lock = lock.clone();
        let hook_target = target.clone();
        BEFORE_LOCK_OPEN.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                symlink(&hook_target, &hook_lock).expect("publish symlink");
            }));
        });

        open_lock_file(&lock).expect_err("a raced symlink must be rejected");
        assert!(
            !target.exists(),
            "the raced symlink target must not be created"
        );
    }

    #[test]
    fn open_lock_file_retries_when_existing_inode_disappears_before_open() {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "tsk-fsperm-lock-removed-{}-{seq}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("mkdir root");
        let _guard = TempDirGuard(root.clone());
        let lock = root.join("lock");
        fs::write(&lock, "old lock").expect("existing lock");
        let hook_lock = lock.clone();
        BEFORE_LOCK_OPEN.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::remove_file(hook_lock).expect("remove between stat and open");
            }));
        });
        let file = open_lock_file(&lock).expect("retry the disappearing inode");
        assert!(file.metadata().expect("opened lock").is_file());
        assert!(lock.is_file());
        file.try_lock().expect("replacement inode is lockable");
    }

    #[test]
    fn open_lock_file_reuses_regular_inode_and_rejects_nonfiles() {
        use std::os::unix::fs::MetadataExt;

        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "tsk-fsperm-lock-inode-{}-{seq}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("mkdir root");
        let _guard = TempDirGuard(root.clone());
        let lock = root.join("lock");
        let first = open_lock_file(&lock).expect("first opener");
        let second = open_lock_file(&lock).expect("second opener");
        let first_metadata = first.metadata().expect("first metadata");
        let second_metadata = second.metadata().expect("second metadata");
        assert_eq!(first_metadata.dev(), second_metadata.dev());
        assert_eq!(first_metadata.ino(), second_metadata.ino());

        fs::create_dir(root.join("not-a-file")).expect("non-file entry");
        open_lock_file(&root.join("not-a-file")).expect_err("non-file lock path must fail");
    }

    #[test]
    fn ensure_private_dir_rejects_a_symlink_without_chmodding_its_target() {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("tsk-fsperm-symlink-{}-{seq}", std::process::id()));
        fs::create_dir_all(&root).expect("mkdir root");
        let _guard = TempDirGuard(root.clone());
        let target = root.join("target");
        let link = root.join("state");
        fs::create_dir(&target).expect("mkdir target");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o500)).expect("chmod target");
        symlink(&target, &link).expect("symlink state dir");

        ensure_private_dir(&link).expect_err("a state directory symlink must be rejected");

        assert_eq!(
            fs::metadata(&target)
                .expect("target metadata")
                .permissions()
                .mode()
                & 0o7777,
            0o500,
            "rejecting the symlink must not grant permissions on its target"
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn junction(link: &Path, target: &Path) {
        let output = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("run mklink");
        assert!(
            output.status.success(),
            "mklink failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn ensure_private_dir_rejects_a_junction_ancestor_without_creating_through_it() {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("tsk-fsperm-junction-{}-{seq}", std::process::id()));
        fs::create_dir_all(&root).expect("root");
        let target = root.join("target");
        let link = root.join("redirect");
        fs::create_dir(&target).expect("target");
        junction(&link, &target);
        assert!(
            is_reparse_or_symlink(&fs::symlink_metadata(&link).expect("junction metadata")),
            "a native junction must carry the reparse-point attribute"
        );

        ensure_private_dir(&link.join("state")).expect_err("junction ancestor must be refused");

        assert!(!target.join("state").exists());
        fs::remove_dir(&link).expect("remove junction");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn replace_file_atomically_replaces_an_existing_destination() {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("tsk-fsperm-replace-{}-{seq}", std::process::id()));
        fs::create_dir_all(&root).expect("root");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, "new").expect("source");
        fs::write(&destination, "old").expect("destination");

        replace_file(&source, &destination).expect("replace");

        assert_eq!(fs::read_to_string(&destination).expect("published"), "new");
        assert!(!source.exists());
        let _ = fs::remove_dir_all(root);
    }
}
