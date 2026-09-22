//! Descriptor-relative setup I/O. Never resolve a mutable parent pathname for writes.
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
};

pub(super) struct Dir {
    file: File,
    pub path: PathBuf,
}
fn name(path: &Path) -> io::Result<CString> {
    if path.components().count() != 1 || path.file_name() != Some(path.as_os_str()) {
        return Err(io::Error::other("expected one child filename"));
    }
    CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)
}
fn checked_fd(fd: i32, path: &Path) -> io::Result<File> {
    if fd < 0 {
        let e = io::Error::last_os_error();
        return Err(io::Error::new(
            e.kind(),
            format!(
                "cannot open {} (symlinks/non-directories refused): {e}",
                path.display()
            ),
        ));
    }
    // SAFETY: a successful open/openat returns a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
impl Dir {
    pub fn open(path: &Path, create: bool) -> io::Result<Self> {
        match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
        {
            Ok(file) => Ok(Self {
                file,
                path: path.into(),
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound && create => {
                let parent = path
                    .parent()
                    .ok_or_else(|| io::Error::other("missing config ancestor"))?;
                Self::open(parent, true)?.child(
                    Path::new(
                        path.file_name()
                            .ok_or_else(|| io::Error::other("missing directory name"))?,
                    ),
                    true,
                )
            }
            Err(e) => Err(io::Error::new(
                e.kind(),
                format!(
                    "cannot open directory {} (symlinks refused): {e}",
                    path.display()
                ),
            )),
        }
    }
    pub fn child(&self, child: &Path, create: bool) -> io::Result<Self> {
        let n = name(child)?;
        let path = self.path.join(child);
        if create {
            // SAFETY: valid live directory descriptor and NUL-terminated single component.
            if unsafe { libc::mkdirat(self.file.as_raw_fd(), n.as_ptr(), 0o700) } != 0 {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::AlreadyExists {
                    return Err(e);
                }
            }
        }
        // SAFETY: same descriptor/name contract; O_NOFOLLOW rejects final links.
        let file = checked_fd(
            unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    n.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            },
            &path,
        )?;
        Ok(Self { file, path })
    }
    fn file(&self, child: &Path, flags: i32) -> io::Result<File> {
        let n = name(child)?;
        // SAFETY: valid descriptor, single component, mode supplied when O_CREAT is set.
        let file = checked_fd(
            unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    n.as_ptr(),
                    flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                    0o600 as libc::c_uint,
                )
            },
            &self.path.join(child),
        )?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("setup file must be a regular file"));
        }
        Ok(file)
    }
    pub fn read(&self, child: &Path) -> io::Result<Option<String>> {
        match self.file(child, libc::O_RDONLY) {
            Ok(mut file) => {
                let mut text = String::new();
                file.read_to_string(&mut text)?;
                Ok(Some(text))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    pub fn write_new(&self, child: &Path, text: &str) -> io::Result<()> {
        let mut file = self.file(child, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    }
    pub fn lock(&self) -> io::Result<File> {
        let file = self.file(Path::new(".tsk-setup.lock"), libc::O_RDWR | libc::O_CREAT)?;
        file.try_lock()
            .map_err(|e| io::Error::other(format!("could not acquire setup lock: {e}")))?;
        // Keep the inode, including after unlock. Removing it lets a second opener lock a new inode.
        Ok(file)
    }
    /// Whether a child name exists (a final symlink counts: rename would clobber its link,
    /// not its target). Pure stat, never creates.
    pub fn exists(&self, child: &Path) -> io::Result<bool> {
        let n = name(child)?;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: valid descriptor, NUL-terminated single component, writable out struct.
        if unsafe {
            libc::fstatat(
                self.file.as_raw_fd(),
                n.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == 0
        {
            return Ok(true);
        }
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::NotFound {
            Ok(false)
        } else {
            Err(e)
        }
    }
    pub fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let from = name(from)?;
        let to = name(to)?;
        // SAFETY: both names are single components anchored to this live directory.
        if unsafe {
            libc::renameat(
                self.file.as_raw_fd(),
                from.as_ptr(),
                self.file.as_raw_fd(),
                to.as_ptr(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        self.file.sync_all()
    }
    pub fn remove(&self, child: &Path, directory: bool) -> io::Result<()> {
        let n = name(child)?;
        // SAFETY: unlinkat never follows a final symlink; no recursive pathname traversal.
        if unsafe {
            libc::unlinkat(
                self.file.as_raw_fd(),
                n.as_ptr(),
                if directory { libc::AT_REMOVEDIR } else { 0 },
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    /// Host commands must receive paths, so reject moved/replaced parents before and after them.
    /// All our own I/O remains descriptor-relative even if a replacement races this check.
    pub fn validate(&self) -> io::Result<()> {
        let current = fs::symlink_metadata(&self.path)?;
        let opened = self.file.metadata()?;
        if !current.is_dir() || current.dev() != opened.dev() || current.ino() != opened.ino() {
            return Err(io::Error::other(format!(
                "config/asset directory changed or became a symlink: {}",
                self.path.display()
            )));
        }
        Ok(())
    }
}
pub(super) struct TempFile<'a> {
    pub dir: &'a Dir,
    pub name: PathBuf,
}
impl TempFile<'_> {
    pub fn preserve(&mut self) {
        self.name = PathBuf::new();
    }
}
impl Drop for TempFile<'_> {
    fn drop(&mut self) {
        if !self.name.as_os_str().is_empty() {
            let _ = self.dir.remove(&self.name, false);
        }
    }
}
