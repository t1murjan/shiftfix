//! Один экземпляр программы: блокировка файла через flock.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

/// Взять блокировку `path`, не дожидаясь. `Ok(None)` — её держит другой
/// процесс. Блокировка действует, пока открыт возвращённый файл.
pub fn try_lock(path: &Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    // SAFETY: flock на собственном открытом дескрипторе.
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    Ok(locked.then_some(file))
}
