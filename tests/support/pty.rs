use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// A fresh scratch root under the system temp dir, unique per process and call.
pub fn scratch_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "tsk-pty-{label}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

/// Run `tsk` with a terminal stdin but captured stdout. This distinguishes the
/// production stdout terminal check from an accidental stdin check.
#[allow(dead_code)]
pub fn run_with_tty_stdin_and_piped_output(
    root: &Path,
    cwd: &Path,
    args: &[&str],
    rows: u16,
    cols: u16,
) -> Output {
    let mut master = -1;
    let mut slave = -1;
    let mut size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty initializes valid descriptors; size is live for the call.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &raw mut size,
            )
        },
        0
    );
    // SAFETY: each descriptor has one owned File after a successful openpty.
    let _master = unsafe { File::from_raw_fd(master) };
    let input = unsafe { File::from_raw_fd(slave) };
    Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(args)
        .current_dir(cwd)
        .env("TSK_STATE_DIR", root.join("state"))
        .env("TSK_NO_UPDATE_CHECK", "1")
        .env_remove("TSK_MODE")
        .env("TERM", "xterm-256color")
        .stdin(Stdio::from(input))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

pub struct Session {
    child: Child,
    tty: File,
    root: PathBuf,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let mut drain = [0; 8192];
        while matches!(self.tty.read(&mut drain), Ok(n) if n > 0) {}
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Session {
    /// Spawn `tsk args...` in `cwd` on a `cols`×`rows` PTY with `TSK_STATE_DIR` under
    /// `root/state` and the update check off. The session owns `root` and removes it on drop.
    pub fn spawn(
        root: PathBuf,
        cwd: &Path,
        args: &[&str],
        envs: &[(&str, &OsStr)],
        rows: u16,
        cols: u16,
    ) -> Session {
        let mut master = -1;
        let mut slave = -1;
        let mut size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: openpty initializes valid descriptors; size is live for the call.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &raw mut size,
                )
            },
            0
        );
        // SAFETY: each descriptor has one owned File, and is valid after openpty.
        let tty = unsafe { File::from_raw_fd(master) };
        let input = unsafe { File::from_raw_fd(slave) };
        // SAFETY: prevent the child from retaining an extra master/slave across exec.
        unsafe {
            assert_ne!(libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC), -1);
            assert_ne!(libc::fcntl(slave, libc::F_SETFD, libc::FD_CLOEXEC), -1);
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_tsk"));
        command
            .args(args)
            .current_dir(cwd)
            .env("TSK_STATE_DIR", root.join("state"))
            .env("TSK_NO_UPDATE_CHECK", "1")
            .env_remove("TSK_MODE")
            .env("TERM", "xterm-256color")
            .stdin(Stdio::from(input.try_clone().unwrap()))
            .stdout(Stdio::from(input.try_clone().unwrap()))
            .stderr(Stdio::from(input));
        for (key, value) in envs {
            command.env(key, value);
        }
        // SAFETY: only async-signal-safe system calls run before exec; stdin is the PTY slave.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        // SAFETY: fcntl changes only this valid owned PTY descriptor's file status flags.
        unsafe {
            let flags = libc::fcntl(tty.as_raw_fd(), libc::F_GETFL);
            assert_ne!(
                libc::fcntl(tty.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK),
                -1
            );
        }
        Session { child, tty, root }
    }

    /// Read the terminal until `needle` paints, panicking with the output after a minute: a
    /// loaded suite can take many seconds to start and paint a debug board.
    pub fn output_until(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut out = String::new();
        while Instant::now() < deadline {
            let mut bytes = [0; 8192];
            if let Ok(n) = self.tty.read(&mut bytes) {
                out.push_str(&String::from_utf8_lossy(&bytes[..n]));
            }
            if out.contains(needle) {
                return out;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("missing {needle}: {out}");
    }

    pub fn send(&mut self, bytes: &[u8]) {
        self.tty.write_all(bytes).unwrap();
    }

    /// Drain output until the child exits, panicking after `timeout`.
    pub fn wait_exit(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            let mut drain = [0; 8192];
            let _ = self.tty.read(&mut drain);
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "process did not exit in {timeout:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
