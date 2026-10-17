//! Чтение и запись config.toml: правка с сохранением комментариев и
//! атомарная замена файла.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Прочитать конфиг: текст и разобранная таблица. Нет файла — `None`.
pub(super) fn read(path: &Path) -> Result<Option<(String, toml::Table)>> {
    if !path.exists() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("не читается {}", path.display()))?;
    let table = text
        .parse()
        .with_context(|| format!("{}: ошибка TOML", path.display()))?;
    Ok(Some((text, table)))
}

/// Записать в `slot` новое значение, оставив комментарии над строкой и после неё.
pub(super) fn set_value(slot: &mut toml_edit::Item, item: &toml_edit::Item) {
    let decor = slot.as_value().map(|v| v.decor().clone());
    *slot = item.clone();
    if let (Some(decor), Some(value)) = (decor, slot.as_value_mut()) {
        *value.decor_mut() = decor;
    }
}

/// Обновить таблицу `key` значениями из `fresh`: комментарии, порядок и лишние
/// ключи файла (например, от более новой версии) остаются. Нет таблицы — новая
/// встаёт после остальных.
pub(super) fn merge_table(doc: &mut toml_edit::DocumentMut, key: &str, fresh: &toml_edit::Table) {
    let Some(table) = doc
        .get_mut(key)
        .and_then(toml_edit::Item::as_table_like_mut)
    else {
        let mut table = fresh.clone();
        table.set_position(None);
        table.decor_mut().clear();
        doc.insert(key, toml_edit::Item::Table(table));
        return;
    };
    for (name, item) in fresh.iter() {
        match table.get_mut(name) {
            Some(slot) => set_value(slot, item),
            None => {
                table.insert(name, item.clone());
            }
        }
    }
}

/// Настоящий файл за симлинком (и висячим тоже): писать надо в него, иначе
/// rename заменит ссылку из dotfiles обычным файлом.
fn real_path(path: &Path) -> PathBuf {
    let mut path = path.to_path_buf();
    // Не больше 40 переходов, как у ядра (ELOOP).
    for _ in 0..40 {
        let Ok(target) = fs::read_link(&path) else {
            break;
        };
        path = match path.parent() {
            Some(dir) => dir.join(target),
            None => target,
        };
    }
    path
}

/// Атомарная запись: временный файл рядом с настоящим и с его правами, затем rename.
pub(super) fn write_atomic(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let path = real_path(path);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).with_context(|| format!("не создаётся {}", dir.display()))?;
    }
    let perms = fs::metadata(&path).ok().map(|m| m.permissions());
    let tmp = path.with_extension("toml.tmp");
    let _ = fs::remove_file(&tmp);
    let written = (|| -> std::io::Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        if let Some(perms) = &perms {
            options.mode(perms.mode() & 0o777);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(text.as_bytes())?;
        if let Some(perms) = perms {
            // Права при создании урезаются umask.
            file.set_permissions(perms)?;
        }
        Ok(())
    })();
    if let Err(err) = written {
        let _ = fs::remove_file(&tmp);
        return Err(err).with_context(|| format!("не записывается {}", tmp.display()));
    }
    fs::rename(&tmp, &path).with_context(|| format!("не записывается {}", path.display()))
}
