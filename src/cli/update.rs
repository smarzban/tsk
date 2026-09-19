//! Installer-aware binary upgrades.

#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(any(unix, windows))]
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(unix)]
use std::{fs, io};

#[cfg(unix)]
const CURL_ENV: &str = "TSK_UPDATE_CURL";
#[cfg(unix)]
const CURL_PATH: &str = "/usr/bin/curl";
#[cfg(unix)]
const INSTALLER_URL: &str = "https://gettsk.sh/install.sh";
#[cfg(windows)]
const INSTALLER_URL: &str = "https://gettsk.sh/install.ps1";
#[cfg(unix)]
const SH_PATH: &str = "/bin/sh";
/// The installer's own pin. `tsk update` always follows the latest published release; an
/// inherited export must not pin or downgrade it.
const INSTALLER_VERSION_ENV: &str = "TSK_VERSION";

/// One curl policy for every Unix fetch the binary makes: HTTPS only (also across
/// redirects), TLS 1.2 or newer, fail on HTTP errors, quiet, follow redirects, bounded time.
#[cfg(unix)]
pub fn hardened_curl(curl: &Path, max_time_secs: u32) -> Command {
    let mut command = Command::new(curl);
    command.args([
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "-fsSL",
        "--max-time",
    ]);
    command.arg(max_time_secs.to_string());
    command
}

/// The curl the Unix release check and `tsk update` share: `TSK_UPDATE_CURL` when set,
/// else the system binary. Never a bare `curl` looked up on `PATH`.
#[cfg(unix)]
pub fn curl_path() -> Result<PathBuf, String> {
    configured_curl_path(std::env::var_os(CURL_ENV).map(PathBuf::from))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOutcome {
    Homebrew,
    Installed,
}

/// Update the running installation. Homebrew owns its formula upgrades; all other
/// installations use the same published-release installer shown in the docs.
#[cfg(unix)]
pub fn run() -> Result<UpdateOutcome, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate the running tsk executable: {error}"))?;
    let curl = curl_path()?;
    run_for(
        &executable,
        &curl,
        Path::new(SH_PATH),
        &std::env::temp_dir(),
    )
}

/// Update a Windows installer-managed copy through the checksum-verifying PowerShell
/// installer. The installer stages `tsk.exe` and starts a detached helper that waits for this
/// process to exit before replacing the running executable.
#[cfg(windows)]
pub fn run() -> Result<UpdateOutcome, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate the running tsk executable: {error}"))?;
    let install_dir = executable
        .parent()
        .ok_or_else(|| "the running tsk executable has no installation directory".to_string())?;
    let script = download_https(INSTALLER_URL, 1024 * 1024, 120)?;
    let powershell = windows_powershell_path()?;
    let mut command = Command::new(&powershell);
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        "$source = [Console]::In.ReadToEnd(); & ([ScriptBlock]::Create($source))",
    ]);
    run_windows_installer(&mut command, &script, install_dir, std::process::id())?;
    Ok(UpdateOutcome::Installed)
}

#[cfg(windows)]
fn run_windows_installer(
    command: &mut Command,
    script: &[u8],
    install_dir: &std::path::Path,
    update_pid: u32,
) -> Result<(), String> {
    let mut child = command
        .stdin(Stdio::piped())
        .env("TSK_INSTALL_DIR", install_dir)
        .env("TSK_UPDATE", "1")
        .env("TSK_UPDATE_PID", update_pid.to_string())
        .env(
            "TSK_CURRENT_VERSION",
            concat!("v", env!("CARGO_PKG_VERSION")),
        )
        .env_remove(INSTALLER_VERSION_ENV)
        .spawn()
        .map_err(|error| format!("could not start the Windows installer: {error}"))?;
    {
        use std::io::Write;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "could not open Windows PowerShell input".to_string())?;
        if let Err(error) = stdin.write_all(script) {
            drop(stdin);
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "could not pass the installer to Windows PowerShell: {error}"
            ));
        }
    }
    let status = child
        .wait()
        .map_err(|error| format!("could not wait for Windows PowerShell: {error}"))?;
    if !status.success() {
        return Err(format!(
            "Windows installer failed (exit {})",
            status
                .code()
                .map_or_else(|| "terminated".to_string(), |code| code.to_string())
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn configured_curl_path(configured: Option<PathBuf>) -> Result<PathBuf, String> {
    match configured {
        Some(path) if path.is_absolute() => Ok(path),
        Some(_) => Err(format!("{CURL_ENV} must be an absolute path")),
        None => Ok(PathBuf::from(CURL_PATH)),
    }
}

#[cfg(unix)]
fn run_for(
    executable: &Path,
    curl: &Path,
    shell: &Path,
    scratch: &Path,
) -> Result<UpdateOutcome, String> {
    let executable = normalized(executable);
    if is_homebrew_install(&executable) {
        return Ok(UpdateOutcome::Homebrew);
    }
    let install_dir = executable
        .parent()
        .ok_or_else(|| "the running tsk executable has no installation directory".to_string())?;
    run_installer(install_dir, curl, shell, scratch)?;
    Ok(UpdateOutcome::Installed)
}

#[cfg(unix)]
fn is_homebrew_install(executable: &Path) -> bool {
    let executable = normalized(executable);
    executable.ancestors().any(|path| {
        path.file_name().is_some_and(|name| name == "tsk")
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "Cellar")
    })
}

/// Download the installer to a private file, and only once curl has finished successfully
/// hand that file to the shell. Streaming `curl | sh` would let a connection that drops
/// mid-script execute the prefix that arrived.
///
/// The shell receives an open descriptor on its stdin, not a path: after curl exits nothing
/// reopens the script by name, so whoever controls `TMPDIR` cannot swap it in between.
/// `install.sh` already reads its prompts from `/dev/tty` when stdin is not a terminal.
#[cfg(unix)]
fn run_installer(
    install_dir: &Path,
    curl: &Path,
    shell: &Path,
    scratch: &Path,
) -> Result<(), String> {
    let script = InstallerFile::create(scratch, "install.sh")?;
    let download = hardened_curl(curl, 120)
        .arg("-o")
        .arg(&script.path)
        .arg(INSTALLER_URL)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| {
            format!(
                "could not start the installer download with {}: {error} (set {CURL_ENV} to an absolute curl path)",
                curl.display()
            )
        })?;
    if !download.success() {
        return Err(format!(
            "could not download {INSTALLER_URL} (exit {})",
            exit_label(download.code())
        ));
    }
    let file = script.open_downloaded()?;
    let installer_status = Command::new(shell)
        .stdin(Stdio::from(file))
        .env("TSK_INSTALL_DIR", install_dir)
        .env("TSK_UPDATE", "1")
        // The installer cannot know what it is replacing; the running binary can.
        .env(
            "TSK_CURRENT_VERSION",
            concat!("v", env!("CARGO_PKG_VERSION")),
        )
        .env_remove(INSTALLER_VERSION_ENV)
        .status()
        .map_err(|error| format!("could not start the installer: {error}"))?;
    if !installer_status.success() {
        return Err(format!(
            "installer failed (exit {})",
            exit_label(installer_status.code())
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_powershell_path() -> Result<PathBuf, String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

    let mut buffer = vec![0_u16; 32_768];
    // SAFETY: buffer is writable for the supplied length. GetSystemDirectoryW writes at most
    // that many UTF-16 code units and does not retain the pointer.
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 {
        return Err(format!(
            "could not locate the Windows system directory: {}",
            std::io::Error::last_os_error()
        ));
    }
    if length as usize >= buffer.len() {
        return Err("the Windows system directory path is too long".to_string());
    }
    buffer.truncate(length as usize);
    let path =
        PathBuf::from(OsString::from_wide(&buffer)).join("WindowsPowerShell/v1.0/powershell.exe");
    if path.is_absolute() && path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "Windows PowerShell was not found at {}",
            path.display()
        ))
    }
}

/// Download one small HTTPS resource with bounded redirects, time, and body size.
#[cfg(windows)]
pub(crate) fn download_https(url: &str, limit: u64, timeout_secs: u64) -> Result<Vec<u8>, String> {
    use std::time::Duration;

    let config = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(5)
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .user_agent(concat!("tsk/", env!("CARGO_PKG_VERSION")))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut response = agent
        .get(url)
        .call()
        .map_err(|error| format!("could not download {url}: {error}"))?;
    let body = response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|error| format!("could not read {url}: {error}"))?;
    if body.is_empty() {
        return Err(format!("downloaded file from {url} is empty"));
    }
    Ok(body)
}

/// A private scratch directory (`0700` on Unix, created with the atomic `mkdir` that fails
/// when the name exists) holding the downloaded installer, removed on drop, success or failure.
///
/// curl and the shell both open the script by name, so the file alone would leave a window
/// between curl's close and the shell's open in which another party writing to the same
/// temp dir could swap it. Only the owner can create or replace entries in this directory,
/// which closes that window regardless of what `TMPDIR` points at.
#[cfg(unix)]
struct InstallerFile {
    dir: PathBuf,
    path: PathBuf,
}

#[cfg(unix)]
impl InstallerFile {
    fn create(scratch: &Path, filename: &str) -> Result<Self, String> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        for _ in 0..64 {
            let dir = scratch.join(format!(
                ".tsk-installer.{}.{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = fs::DirBuilder::new();
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&dir) {
                Ok(()) => {
                    return Ok(Self {
                        path: dir.join(filename),
                        dir,
                    })
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!(
                        "could not create the installer directory in {}: {error}",
                        scratch.display()
                    ))
                }
            }
        }
        Err(format!(
            "could not create a unique installer directory in {}",
            scratch.display()
        ))
    }
}

#[cfg(unix)]
impl InstallerFile {
    /// Open the script curl wrote, without following a symlink, and check it is a regular,
    /// non-empty file owned by this user before it is handed to the shell.
    fn open_downloaded(&self) -> Result<fs::File, String> {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(&self.path)
            .map_err(|error| format!("could not read the downloaded installer: {error}"))?;
        let meta = file
            .metadata()
            .map_err(|error| format!("could not read the downloaded installer: {error}"))?;
        if !meta.file_type().is_file() {
            return Err("downloaded installer is not a regular file".to_string());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // SAFETY: getuid has no preconditions and cannot fail.
            if meta.uid() != unsafe { libc::getuid() } {
                return Err("downloaded installer is not owned by this user".to_string());
            }
        }
        if meta.len() == 0 {
            return Err(format!(
                "downloaded installer from {INSTALLER_URL} is empty"
            ));
        }
        Ok(file)
    }
}

#[cfg(unix)]
impl Drop for InstallerFile {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[cfg(unix)]
fn exit_label(code: Option<i32>) -> String {
    code.map_or_else(|| "signal".to_string(), |code| code.to_string())
}

#[cfg(unix)]
fn normalized(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    #[cfg(any(unix, windows))]
    use std::fs;
    #[cfg(windows)]
    use std::io::Read;
    #[cfg(unix)]
    use std::path::{Path, PathBuf};
    #[cfg(windows)]
    use std::process::Command;
    #[cfg(any(unix, windows))]
    use std::sync::atomic::{AtomicU64, Ordering};

    #[cfg(unix)]
    use super::{configured_curl_path, is_homebrew_install, run_for, UpdateOutcome};
    #[cfg(windows)]
    use super::{run_windows_installer, windows_powershell_path};

    #[cfg(any(unix, windows))]
    static SEQ: AtomicU64 = AtomicU64::new(0);

    #[cfg(unix)]
    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tsk-update-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create temporary directory");
        dir
    }

    #[cfg(unix)]
    fn command(dir: &Path, name: &str, source: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, source).expect("write test command");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("make test command executable");
        }
        path
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "helper process for windows_installer_handoff_streams_the_whole_script_and_sets_update_environment"]
    fn windows_installer_capture_child() {
        let mut script = Vec::new();
        std::io::stdin()
            .read_to_end(&mut script)
            .expect("read installer stdin");
        fs::write(
            std::env::var_os("FAKE_SCRIPT").expect("FAKE_SCRIPT"),
            script,
        )
        .expect("write captured script");
        let version = std::env::var("TSK_VERSION").unwrap_or_else(|_| "unset".to_string());
        fs::write(
            std::env::var_os("FAKE_ENV").expect("FAKE_ENV"),
            format!(
                "{}|{}|{}|{}|{}",
                std::env::var("TSK_INSTALL_DIR").expect("TSK_INSTALL_DIR"),
                std::env::var("TSK_UPDATE").expect("TSK_UPDATE"),
                std::env::var("TSK_UPDATE_PID").expect("TSK_UPDATE_PID"),
                std::env::var("TSK_CURRENT_VERSION").expect("TSK_CURRENT_VERSION"),
                version
            ),
        )
        .expect("write captured environment");
    }

    #[cfg(windows)]
    #[test]
    fn windows_installer_handoff_streams_the_whole_script_and_sets_update_environment() {
        let dir = std::env::temp_dir().join(format!(
            "tsk-update-windows-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("temporary directory");
        let fake = dir.join("fake-installer.cmd");
        let script_copy = dir.join("script.bin");
        let env_log = dir.join("environment.txt");
        let current_test = std::env::current_exe().expect("current test executable");
        fs::write(
            &fake,
            format!(
                "@echo off\r\n\"{}\" --ignored --exact cli::update::tests::windows_installer_capture_child --nocapture\r\nexit /b %ERRORLEVEL%\r\n",
                current_test.display()
            ),
        )
        .expect("fake command");
        let install_dir = dir.join("installed bin");
        let script = b"first line\r\nsecond line\r\n";
        let mut command = Command::new(&fake);
        command
            .env("FAKE_SCRIPT", &script_copy)
            .env("FAKE_ENV", &env_log)
            .env("TSK_VERSION", "v0.0.1");

        run_windows_installer(&mut command, script, &install_dir, 4242).expect("handoff");

        assert_eq!(fs::read(&script_copy).expect("script bytes"), script);
        assert_eq!(
            fs::read_to_string(&env_log)
                .expect("environment")
                .trim_end(),
            format!(
                "{}|1|4242|v{}|unset",
                install_dir.display(),
                env!("CARGO_PKG_VERSION")
            )
        );
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[cfg(windows)]
    #[test]
    fn windows_updater_uses_the_absolute_system_powershell() {
        let path = windows_powershell_path().expect("stock Windows PowerShell");
        assert!(path.is_absolute());
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("powershell.exe")
        );
    }

    #[cfg(unix)]
    #[test]
    fn homebrew_install_is_identified_without_brew_on_path() {
        assert!(is_homebrew_install(Path::new(
            "/opt/homebrew/Cellar/tsk/0.7.0/bin/tsk"
        )));
        assert!(is_homebrew_install(Path::new(
            "/home/linuxbrew/.linuxbrew/Cellar/tsk/0.7.0/bin/tsk"
        )));
        assert!(!is_homebrew_install(Path::new(
            "/Users/alex/.local/bin/tsk"
        )));
    }

    #[cfg(unix)]
    #[test]
    fn nonstandard_curl_path_must_be_explicit_and_absolute() {
        assert_eq!(
            configured_curl_path(Some(PathBuf::from("/opt/tools/curl"))),
            Ok(PathBuf::from("/opt/tools/curl"))
        );
        assert_eq!(
            configured_curl_path(Some(PathBuf::from("curl"))),
            Err("TSK_UPDATE_CURL must be an absolute path".into())
        );
    }

    /// A curl stand-in that records its argv, then writes `payload` to the `-o` target
    /// and exits with `exit`.
    #[cfg(unix)]
    fn fake_curl(dir: &Path, payload: &str, exit: u8) -> (PathBuf, PathBuf) {
        let argv = dir.join("curl-argv");
        let curl = command(
            dir,
            "curl",
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nout=\nwhile [ $# -gt 0 ]; do if [ \"$1\" = -o ]; then out=$2; shift; fi; shift; done\nls -ld \"$(dirname \"$out\")\" | cut -c1-10 > '{}'\nprintf '%s' '{payload}' > \"$out\"\nexit {exit}\n",
                argv.display(),
                dir.join("curl-dir-mode").display()
            ),
        );
        (curl, argv)
    }

    /// A shell stand-in that records the script it receives on stdin (a path argument is
    /// a failure: the handoff must be by descriptor) and the environment.
    #[cfg(unix)]
    fn fake_sh(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let script_copy = dir.join("installer-input");
        let env_log = dir.join("installer-env");
        let shell = command(
            dir,
            "sh",
            &format!(
                "#!/bin/sh\n[ $# -eq 0 ] || exit 99\ncat > '{}'\nprintf '%s|%s|%s|%s' \"$TSK_INSTALL_DIR\" \"$TSK_UPDATE\" \"$TSK_CURRENT_VERSION\" \"${{TSK_VERSION-unset}}\" > '{}'\n",
                script_copy.display(),
                env_log.display()
            ),
        );
        (shell, script_copy, env_log)
    }

    #[cfg(unix)]
    fn installer_files(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .expect("scratch dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".tsk-installer."))
            })
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn installer_is_downloaded_whole_then_run_with_pinned_tools_and_a_clean_environment() {
        let dir = temp_dir("installer");
        let (curl, argv) = fake_curl(&dir, "installer-payload", 0);
        let (shell, script_copy, env_log) = fake_sh(&dir);
        let executable = dir.join("custom/bin/tsk");
        fs::create_dir_all(executable.parent().expect("executable parent"))
            .expect("create custom install directory");

        // A stale pin in the caller's shell must never reach the installer.
        std::env::set_var("TSK_VERSION", "v0.0.1");
        let outcome = run_for(&executable, &curl, &shell, &dir);
        std::env::remove_var("TSK_VERSION");
        assert_eq!(outcome, Ok(UpdateOutcome::Installed));

        assert_eq!(
            fs::read_to_string(&script_copy).expect("installer received download"),
            "installer-payload"
        );
        let env_log = fs::read_to_string(&env_log).expect("installer environment");
        let mut fields = env_log.split('|');
        assert_eq!(
            fields.next(),
            Some(
                executable
                    .parent()
                    .expect("executable parent")
                    .display()
                    .to_string()
                    .as_str()
            )
        );
        // The installer prints "Current version" only from this handoff.
        assert_eq!(fields.next(), Some("1"));
        assert_eq!(fields.next(), Some(concat!("v", env!("CARGO_PKG_VERSION"))));
        assert_eq!(fields.next(), Some("unset"), "TSK_VERSION must be stripped");

        let argv = fs::read_to_string(&argv).expect("curl argv");
        let args: Vec<&str> = argv.lines().collect();
        assert_eq!(
            &args[..8],
            &[
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--tlsv1.2",
                "-fsSL",
                "--max-time",
                "120"
            ]
        );
        assert_eq!(args[8], "-o");
        assert_eq!(args[10], "https://gettsk.sh/install.sh");
        assert!(
            args[9].starts_with(&dir.join(".tsk-installer.").display().to_string()),
            "script lives in a private directory under the scratch dir: {}",
            args[9]
        );
        assert_eq!(
            fs::read_to_string(dir.join("curl-dir-mode"))
                .expect("directory mode")
                .trim(),
            "drwx------",
            "the installer directory is private while curl writes into it"
        );
        assert!(
            installer_files(&dir).is_empty(),
            "the downloaded script is removed after the run"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_or_empty_download_never_reaches_the_shell() {
        let dir = temp_dir("installer-truncated");
        let executable = dir.join("custom/bin/tsk");
        fs::create_dir_all(executable.parent().expect("executable parent"))
            .expect("create custom install directory");

        // curl wrote a prefix of the script, then died: the shell must not see it.
        let (curl, _) = fake_curl(&dir, "#!/bin/sh\nrm -rf", 56);
        let (shell, script_copy, _) = fake_sh(&dir);
        let error =
            run_for(&executable, &curl, &shell, &dir).expect_err("truncated download fails");
        assert!(error.contains("could not download"), "{error}");
        assert!(error.contains("exit 56"), "{error}");
        assert!(!script_copy.exists(), "shell ran on a truncated download");

        // A 200 with an empty body is refused too.
        let (curl, _) = fake_curl(&dir, "", 0);
        let error = run_for(&executable, &curl, &shell, &dir).expect_err("empty download fails");
        assert!(error.contains("is empty"), "{error}");
        assert!(!script_copy.exists(), "shell ran on an empty download");

        assert!(installer_files(&dir).is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_script_swapped_for_a_symlink_after_download_is_refused() {
        let dir = temp_dir("installer-swapped");
        let executable = dir.join("custom/bin/tsk");
        fs::create_dir_all(executable.parent().expect("executable parent"))
            .expect("create custom install directory");
        let target = dir.join("elsewhere.sh");
        fs::write(&target, "#!/bin/sh\nexit 0\n").expect("symlink target");
        // curl "succeeds" but what sits at the path afterwards is a symlink.
        let curl = command(
            &dir,
            "curl",
            &format!(
                "#!/bin/sh\nout=\nwhile [ $# -gt 0 ]; do if [ \"$1\" = -o ]; then out=$2; shift; fi; shift; done\nln -s '{}' \"$out\"\nexit 0\n",
                target.display()
            ),
        );
        let (shell, script_copy, _) = fake_sh(&dir);
        let error = run_for(&executable, &curl, &shell, &dir).expect_err("symlink refused");
        assert!(
            error.contains("could not read the downloaded installer")
                || error.contains("not a regular file"),
            "{error}"
        );
        assert!(!script_copy.exists(), "shell ran on a swapped script");
        assert!(installer_files(&dir).is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_curl_names_the_override() {
        let dir = temp_dir("installer-nocurl");
        let executable = dir.join("custom/bin/tsk");
        fs::create_dir_all(executable.parent().expect("executable parent"))
            .expect("create custom install directory");
        let (shell, _, _) = fake_sh(&dir);
        let error = run_for(&executable, &dir.join("absent-curl"), &shell, &dir)
            .expect_err("missing curl fails");
        assert!(error.contains("TSK_UPDATE_CURL"), "{error}");
        let _ = fs::remove_dir_all(dir);
    }
}
