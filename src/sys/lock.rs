//! Один экземпляр программы: блокировка файла (flock).

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// Взять блокировку `path`, не дожидаясь. `Ok(None)` — её держит другой
/// процесс. Блокировка действует, пока открыт возвращённый файл.
pub fn try_lock(path: &Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    Ok(file.try_lock().is_ok().then_some(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_fails_until_first_is_closed() {
        let dir = crate::testutil::TempDir::new("lock");
        let path = dir.join("x.lock");
        let first = try_lock(&path).unwrap();
        assert!(first.is_some());
        assert!(try_lock(&path).unwrap().is_none());
        drop(first);
        assert!(try_lock(&path).unwrap().is_some());
    }
}
