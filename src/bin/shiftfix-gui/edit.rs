//! Правка настроек в окне: значения на экране, значения в файле и ошибка
//! чтения файла.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use shiftfix::config::{Settings, ThemeSettings};
use shiftfix::devices::Role;

pub struct SettingsEdit {
    path: PathBuf,
    /// Значения на экране: их меняют страницы.
    pub current: Settings,
    /// Значения в файле: прочитанные или записанные окном последними.
    saved: Settings,
    /// Файл не читается или в нём неверное значение: на экране умолчания, а
    /// не значения из файла. Пока ошибку не исправят и файл не перечитают
    /// (`reload`), окно его не пишет — иначе значения пользователя
    /// заменились бы умолчаниями.
    error: Option<String>,
}

impl SettingsEdit {
    /// Прочитать `path`. Ошибка в файле — умолчания и `error`, но тему всё
    /// равно берём из файла: её разбор мягкий.
    pub fn load(path: PathBuf) -> SettingsEdit {
        let (settings, error) = match Settings::load(&path) {
            Ok(s) => (s, None),
            Err(err) => {
                let theme = ThemeSettings::load(&path).unwrap_or_default();
                let settings = Settings {
                    theme,
                    ..Settings::default()
                };
                (settings, Some(format!("{err:#}")))
            }
        };
        SettingsEdit {
            path,
            saved: settings.clone(),
            current: settings,
            error,
        }
    }

    /// Перечитать файл, например после правки руками. Несохранённые правки
    /// пропадают, тема остаётся: её окно пишет само.
    pub fn reload(&mut self) {
        let theme = std::mem::take(&mut self.current.theme);
        *self = SettingsEdit::load(std::mem::take(&mut self.path));
        self.set_theme(theme);
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn saved(&self) -> &Settings {
        &self.saved
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Есть несохранённые правки.
    pub fn dirty(&self) -> bool {
        self.current != self.saved
    }

    /// Вернуть на экран значения из файла.
    pub fn revert(&mut self) {
        self.current = self.saved.clone();
    }

    /// Значения по умолчанию на экране, записывает их «Сохранить». Роли
    /// устройств и оформление сбрасываются отдельно.
    pub fn reset_to_defaults(&mut self) {
        self.current = Settings {
            devices: std::mem::take(&mut self.current.devices),
            theme: std::mem::take(&mut self.current.theme),
            ..Settings::default()
        };
    }

    /// Тему окно пишет само (`ThemeState`), без «Сохранить»: она сразу и на
    /// экране, и в файле — несохранённой правкой не считается.
    pub fn set_theme(&mut self, theme: ThemeSettings) {
        self.saved.theme = theme.clone();
        self.current.theme = theme;
    }

    /// Записать правки с экрана — только изменённые параметры: правки файла
    /// руками в остальных остаются.
    pub fn save(&mut self) -> Result<()> {
        self.read()?;
        self.current.save_changes(&self.saved, &self.path)?;
        self.saved = self.current.clone();
        Ok(())
    }

    /// Роль устройства вручную (`None` — снова автоматически). Файл
    /// перечитывается, и меняется в нём только `[devices]`; несохранённые
    /// правки остаются на экране.
    pub fn set_role(&mut self, key: &str, role: Option<Role>) -> Result<()> {
        let on_disk = self.read()?;
        let mut changed = on_disk.clone();
        put_role(&mut changed.devices, key, role);
        changed.save_changes(&on_disk, &self.path)?;
        self.saved.devices = changed.devices.clone();
        self.current.devices = changed.devices;
        Ok(())
    }

    /// Файл как он есть сейчас. Ошибка в нём запоминается: пока файл не
    /// перечитают (`reload`), окно его не пишет.
    fn read(&mut self) -> Result<Settings> {
        if let Some(err) = &self.error {
            bail!("{err}");
        }
        Settings::load(&self.path).inspect_err(|err| self.error = Some(format!("{err:#}")))
    }
}

fn put_role(devices: &mut BTreeMap<String, String>, key: &str, role: Option<Role>) {
    match role {
        Some(role) => devices.insert(key.to_owned(), role.as_str().to_owned()),
        None => devices.remove(key),
    };
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// config.toml из `text` в своём каталоге; каталог удаляет `cleanup`.
    fn config(tag: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shiftfix-edit-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, text).unwrap();
        path
    }

    fn cleanup(path: &Path) {
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn dirty_revert_and_defaults() {
        let path = config(
            "dirty",
            "double_tap_ms = 350\n[devices]\n\"1:2 Pad\" = \"mouse\"\n",
        );
        let mut edit = SettingsEdit::load(path.clone());
        assert_eq!(edit.error(), None);
        assert!(!edit.dirty());

        edit.current.double_tap_ms = 500;
        assert!(edit.dirty());
        edit.revert();
        assert!(!edit.dirty());
        assert_eq!(edit.current.double_tap_ms, 350);

        // Тема пишется отдельно и несохранённой правкой не считается.
        edit.set_theme(ThemeSettings {
            dark_accent: "#123456".into(),
            ..ThemeSettings::default()
        });
        assert!(!edit.dirty());

        // «По умолчанию» не трогает роли и тему.
        edit.reset_to_defaults();
        assert!(edit.dirty());
        assert_eq!(
            edit.current.double_tap_ms,
            Settings::default().double_tap_ms
        );
        assert_eq!(edit.current.devices, edit.saved().devices);
        assert_eq!(edit.current.theme.dark_accent, "#123456");
        cleanup(&path);
    }

    #[test]
    fn role_keeps_unsaved_edits() {
        let path = config("role", "double_tap_ms = 350\n");
        let mut edit = SettingsEdit::load(path.clone());
        edit.current.double_tap_ms = 500;
        edit.set_role("1:2 Pad", Some(Role::Touchscreen)).unwrap();

        let on_disk = Settings::load(&path).unwrap();
        assert_eq!(on_disk.devices["1:2 Pad"], "touchscreen");
        assert_eq!(
            on_disk.double_tap_ms, 350,
            "несохранённая правка — не в файле"
        );
        assert_eq!(edit.current.double_tap_ms, 500, "и не пропала с экрана");
        assert_eq!(edit.current.devices, on_disk.devices);
        assert!(edit.dirty());

        edit.set_role("1:2 Pad", None).unwrap();
        assert!(Settings::load(&path).unwrap().devices.is_empty());
        cleanup(&path);
    }

    #[test]
    fn broken_value_is_not_replaced_by_defaults() {
        // Неверное значение в файле: на экране умолчания, и ни «Сохранить»,
        // ни роль устройства не должны записать их поверх значений пользователя.
        let text = "double_tap_ms = 350\nkey_delay_ms = \"x\"\n";
        let path = config("broken", text);
        let mut edit = SettingsEdit::load(path.clone());
        assert!(edit.error().is_some());
        edit.current.max_word_len = 10;
        assert!(edit.save().is_err());
        assert!(edit.set_role("1:2 Pad", Some(Role::Mouse)).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), text);

        // Исправили руками и перечитали — запись снова работает.
        fs::write(&path, "double_tap_ms = 350\nkey_delay_ms = 7\n").unwrap();
        edit.reload();
        assert_eq!(edit.error(), None);
        assert_eq!(edit.current.double_tap_ms, 350);
        edit.set_role("1:2 Pad", Some(Role::Mouse)).unwrap();
        let on_disk = Settings::load(&path).unwrap();
        assert_eq!((on_disk.double_tap_ms, on_disk.key_delay_ms), (350, 7));

        // Сломали, пока окно открыто: запись отказывается и запоминает ошибку.
        fs::write(&path, text).unwrap();
        edit.current.max_word_len = 10;
        assert!(edit.save().is_err());
        assert!(edit.error().is_some());
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        cleanup(&path);
    }

    #[test]
    fn writes_keep_edits_by_hand() {
        // Файл поправили руками, пока окно открыто: роль меняет только
        // [devices], «Сохранить» — только то, что изменили в окне.
        let path = config("by-hand", "double_tap_ms = 350\n");
        let mut edit = SettingsEdit::load(path.clone());
        let by_hand = "double_tap_ms = 360\nmax_word_len = 20\n";
        fs::write(&path, by_hand).unwrap();
        edit.set_role("1:2 Pad", Some(Role::Mouse)).unwrap();
        assert!(fs::read_to_string(&path).unwrap().starts_with(by_hand));

        edit.current.key_delay_ms = 9;
        edit.save().unwrap();
        assert!(!edit.dirty());
        let on_disk = Settings::load(&path).unwrap();
        assert_eq!(
            (
                on_disk.double_tap_ms,
                on_disk.max_word_len,
                on_disk.key_delay_ms
            ),
            (360, 20, 9)
        );
        assert_eq!(on_disk.devices["1:2 Pad"], "mouse");
        cleanup(&path);
    }
}
