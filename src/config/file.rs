//! Чтение и запись config.toml: правка с сохранением комментариев и
//! атомарная замена файла.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::DEFAULT_CONFIG;

/// Текст конфига; нет файла — `None`.
fn read_text(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("не читается {}", path.display())),
    }
}

/// Прочитать конфиг: текст и разобранная таблица. Нет файла — `None`.
pub(super) fn read(path: &Path) -> Result<Option<(String, toml::Table)>> {
    let Some(text) = read_text(path)? else {
        return Ok(None);
    };
    let table = text
        .parse()
        .with_context(|| format!("{}: ошибка TOML", path.display()))?;
    Ok(Some((text, table)))
}

/// Правка конфига с сохранением комментариев: `change` меняет документ, файл
/// пишется, только если текст изменился. Нет файла — основа DEFAULT_CONFIG;
/// файл не читается или не разбирается — `Err`, и файл остаётся как был:
/// собрать его заново значило бы потерять всё, чего нет в правке.
pub(super) fn edit(path: &Path, change: impl FnOnce(&mut toml_edit::DocumentMut)) -> Result<()> {
    let original = read_text(path)?;
    let mut doc: toml_edit::DocumentMut = original
        .as_deref()
        .unwrap_or(DEFAULT_CONFIG)
        .parse()
        .with_context(|| format!("{}: ошибка TOML", path.display()))?;
    change(&mut doc);
    let text = doc.to_string();
    if original.as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    write_atomic(path, &text)
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

/// Временный файл для записи `path`: рядом с ним, к имени дописано «.tmp».
pub(super) fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Атомарная запись: временный файл рядом с настоящим и с его правами, затем rename.
/// Перед rename данные сбрасываются на диск: иначе после сбоя питания на месте
/// конфига мог бы остаться пустой файл.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let path = real_path(path);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).with_context(|| format!("не создаётся {}", dir.display()))?;
    }
    let perms = fs::metadata(&path).ok().map(|m| m.permissions());
    let tmp = tmp_path(&path);
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
        file.sync_all()
    })();
    if let Err(err) = written {
        let _ = fs::remove_file(&tmp);
        return Err(err).with_context(|| format!("не записывается {}", tmp.display()));
    }
    fs::rename(&tmp, &path).with_context(|| format!("не записывается {}", path.display()))
}
