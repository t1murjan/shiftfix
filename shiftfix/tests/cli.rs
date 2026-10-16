//! CLI службы глазами install.sh.

use std::process::Command;

use shiftfix::config::{Settings, ThemeSettings};

/// install.sh создаёт конфиг из `--print-config`: он должен разбираться в
/// значения по умолчанию, включая [theme].
#[test]
fn print_config_parses_to_defaults() {
    let out = Command::new(env!("CARGO_BIN_EXE_shiftfix"))
        .arg("--print-config")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        toml::from_str::<Settings>(&text).unwrap(),
        Settings::default()
    );
    let table: toml::Table = text.parse().unwrap();
    assert_eq!(
        ThemeSettings::from_value(&table["theme"]),
        ThemeSettings::default()
    );
}
