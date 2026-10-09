use std::os::unix::{
    fs::{symlink, PermissionsExt},
    process::CommandExt,
};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub struct Host {
    pub root: PathBuf,
    pub config: PathBuf,
    pub bin: PathBuf,
}
impl Host {
    pub fn new(binary: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "tsk-setup-host-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir(root.join("config")).unwrap();
        fs::create_dir(root.join("outside")).unwrap();
        let bin = root.join("bin");
        symlink(binary, bin.join("tsk")).unwrap();
        fs::write(bin.join("herdr"), r#"#!/bin/sh
printf '%s\n' "$*" >> "$FIXTURE/calls"
case "$1 $2" in
  '--version ')
    printf 'herdr %s\n' "${HERDR_VERSION:-0.9.0}";;
  'config check')
    printf '%s' "$HERDR_CONFIG_PATH" > "$FIXTURE/checked"
    case "$SCENARIO" in
      invalid) printf '\033]52;c;bad\007invalid config\n  second diagnostic line\n' >&2; exit 1;;
      change) printf '# external edit\n' > "$FIXTURE/config/config.toml";;
      swap) mv "$FIXTURE/config" "$FIXTURE/moved"; ln -s "$FIXTURE/outside" "$FIXTURE/config";;
      block) touch "$FIXTURE/waiting"; while [ -d "$FIXTURE" ] && [ ! -f "$FIXTURE/release" ]; do sleep 0.02; done;;
    esac;;
  'plugin list')
    if [ -f "$FIXTURE/registry" ]; then cat "$FIXTURE/registry"; else printf '{"result":{"plugins":[]}}'; fi;;
  'plugin link')
    if [ "$SCENARIO" = link-fail ]; then echo refused >&2; exit 1; fi
    printf '{"result":{"plugins":[{"plugin_id":"herdr-tsk","plugin_root":"%s"}]}}' "$3" > "$FIXTURE/registry"
    printf '%s' "$3" > "$FIXTURE/linked"
    if [ "$SCENARIO" = change-link ]; then printf '# external edit\n' > "$FIXTURE/config/config.toml"; fi;;
esac
"#).unwrap();
        fs::set_permissions(bin.join("herdr"), fs::Permissions::from_mode(0o755)).unwrap();
        let config = root.join("config/config.toml");
        Self { root, config, bin }
    }
    pub fn command(&self) -> Command {
        let mut command = Command::new(self.bin.join("tsk"));
        command
            .arg0("tsk")
            .args(["setup", "herdr"])
            .env(
                "PATH",
                format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("HERDR_CONFIG_PATH", &self.config)
            .env("FIXTURE", &self.root)
            .stdin(std::process::Stdio::null());
        command
    }
    pub fn run(&self, scenario: &str) -> Output {
        self.command().env("SCENARIO", scenario).output().unwrap()
    }
    pub fn check(&self) -> Output {
        self.command().arg("--check").output().unwrap()
    }
    pub fn calls(&self) -> String {
        fs::read_to_string(self.root.join("calls")).unwrap_or_default()
    }
    pub fn linked(&self) -> PathBuf {
        fs::read_to_string(self.root.join("linked")).unwrap().into()
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
