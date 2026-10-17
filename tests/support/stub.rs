//! Executable stubs for git, a shell, or tsk to run. Also compiled into the lib's unit
//! tests (`src/lib.rs`), so it depends on std only.
use std::fs;
use std::path::Path;
use std::process::Command;

/// Writes an executable that another process will exec. A file this process wrote is
/// unsafe to exec on Linux: a sibling test thread may fork while the write descriptor is
/// open, and the forked child holds it until its own exec, so the exec fails with ETXTBSY
/// (`spawn_fresh_copy` can retry an exec the test performs itself, but not one inside git
/// or a shell). So this process writes only a staging copy, and a child `install` creates
/// the stub: no descriptor that ever wrote the stub's inode exists in this process.
pub fn write_stub(path: &Path, contents: impl AsRef<[u8]>, mode: u32) {
    let name = path.file_name().expect("stub path has a file name");
    let staging = path.with_file_name(format!(".{}.stub-staging", name.to_string_lossy()));
    fs::write(&staging, contents).expect("write stub staging copy");
    let output = Command::new("install")
        .arg("-m")
        .arg(format!("{mode:o}"))
        .arg(&staging)
        .arg(path)
        .output()
        .expect("run install");
    assert!(
        output.status.success(),
        "install {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_file(&staging).expect("remove stub staging copy");
}
