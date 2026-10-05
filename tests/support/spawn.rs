use std::io;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

/// Spawns a binary that this process just wrote. Another test thread may fork while the
/// copy's write descriptor is still open; the forked child holds it until its own exec,
/// and executing the file meanwhile fails with ETXTBSY. The window is brief, so retry.
/// Only covers an exec the test performs itself: a stub that git, a shell, or tsk execs
/// fails inside that child, where no retry here can reach it.
pub fn spawn_fresh_copy(command: &mut Command) -> io::Result<Child> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match command.spawn() {
            Err(error)
                if error.kind() == io::ErrorKind::ExecutableFileBusy
                    && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(20));
            }
            result => return result,
        }
    }
}
