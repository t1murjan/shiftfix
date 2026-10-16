//! Инструкция.

use eframe::egui::{Align, Color32, CornerRadius, Grid, Pos2, RichText, Sense, Ui, vec2};
use egui_phosphor::regular as icons;

use crate::app::{App, Page, VERSION, shown_path, versions_differ};
use crate::link::ServiceAction;
use crate::theme::Palette;
use crate::widgets::{self, bold, card, card_title, keycap, page_header};

fn number(ui: &mut Ui, p: &Palette, n: usize) {
    let (rect, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 12.0, p.accent.gamma_multiply(0.18));
    ui.painter().text(
        rect.center(),
        eframe::egui::Align2::CENTER_CENTER,
        n.to_string(),
        eframe::egui::FontId::proportional(13.5),
        p.accent,
    );
}

/// Шаг с текстом: перенос строк выравнивается по тексту, а не под номер.
fn step_text(ui: &mut Ui, p: &Palette, n: usize, s: &str) {
    ui.horizontal(|ui| {
        number(ui, p, n);
        ui.add_space(4.0);
        ui.add(eframe::egui::Label::new(RichText::new(s).color(p.text)).wrap());
    });
    ui.add_space(4.0);
}

/// Номер шага в кружке и строка из текста и клавиш рядом.
fn step(ui: &mut Ui, p: &Palette, n: usize, add: impl FnOnce(&mut Ui)) {
    ui.horizontal_wrapped(|ui| {
        number(ui, p, n);
        ui.add_space(4.0);
        add(ui);
    });
    ui.add_space(4.0);
}

fn text(ui: &mut Ui, p: &Palette, s: &str) {
    ui.label(RichText::new(s).color(p.text));
}

fn weak(ui: &mut Ui, p: &Palette, s: &str) {
    ui.add(eframe::egui::Label::new(RichText::new(s).color(p.weak)).wrap());
}

fn bullet(ui: &mut Ui, p: &Palette, s: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("•").color(p.accent));
        ui.add(eframe::egui::Label::new(RichText::new(s).color(p.text)).wrap());
    });
}

/// Пример «было → стало».
fn example(ui: &mut Ui, p: &Palette, before: &str, after: &str) {
    ui.horizontal(|ui| {
        let mono = |s: &str| RichText::new(s).monospace().size(15.0);
        let frame = |ui: &mut Ui, s: &str, color: Color32| {
            let galley = ui.painter().layout_no_wrap(
                s.to_owned(),
                eframe::egui::FontId::monospace(15.0),
                color,
            );
            let (rect, _) =
                ui.allocate_exact_size(galley.size() + vec2(18.0, 10.0), Sense::hover());
            ui.painter()
                .rect_filled(rect, CornerRadius::same(8), p.field);
            ui.painter().galley(
                Pos2::new(rect.left() + 9.0, rect.top() + 5.0),
                galley,
                color,
            );
        };
        frame(ui, before, p.weak);
        keycap(ui, p, "Shift");
        keycap(ui, p, "Shift");
        ui.label(mono("→").color(p.weak));
        frame(ui, after, p.text);
    });
}

pub fn show(app: &mut App, ui: &mut Ui, p: &Palette) {
    page_header(
        ui,
        p,
        icons::BOOK_OPEN_TEXT,
        "Инструкция",
        "Как пользоваться shiftfix и что делать, если что-то не работает",
    );

    card(ui, p, |ui| {
        card_title(ui, p, icons::LIGHTNING, "Коротко");
        text(
            ui,
            p,
            "Набрали текст не в той раскладке — дважды быстро нажмите Shift. Программа сама ничего не переключает: только по вашей команде.",
        );
        ui.add_space(6.0);
        example(ui, p, "ghbdtn", "привет");
        example(ui, p, "руддщ", "hello");
    });
    ui.add_space(12.0);

    card(ui, p, |ui| {
        card_title(ui, p, icons::TEXT_AA, "Исправить последнее слово");
        step_text(
            ui,
            p,
            1,
            "Наберите слово — например, «ghbdtn». Пробел после него не мешает.",
        );
        step(ui, p, 2, |ui| {
            text(ui, p, "Дважды нажмите");
            keycap(ui, p, "Shift");
            text(ui, p, "(левый или правый, быстро, ничего не удерживая).");
        });
        step_text(
            ui,
            p,
            3,
            "Слово сотрётся и наберётся заново в следующей раскладке, раскладка тоже переключится.",
        );
        weak(
            ui,
            p,
            "Нажмёте ещё раз — слово вернётся. Если раскладок три и больше, повторные нажатия перебирают их по кругу.",
        );
        weak(
            ui,
            p,
            "Слово запоминается с момента последнего клика, стрелки или Enter. Вернулись курсором в середину текста — выделите слово (см. ниже).",
        );
    });
    ui.add_space(12.0);

    card(ui, p, |ui| {
        card_title(ui, p, icons::SELECTION, "Исправить выделенный текст");
        step(ui, p, 1, |ui| {
            text(ui, p, "Выделите текст мышью или клавишами");
            keycap(ui, p, "Shift");
            text(ui, p, "+");
            keycap(ui, p, "←");
            keycap(ui, p, "→");
            text(ui, p, ", можно и");
            keycap(ui, p, "Ctrl");
            text(ui, p, "+");
            keycap(ui, p, "A");
        });
        step(ui, p, 2, |ui| {
            text(ui, p, "Дважды нажмите");
            keycap(ui, p, "Shift");
            text(ui, p, "— выделение заменится исправленным текстом.");
        });
        weak(
            ui,
            p,
            "Направление определяется само: латиница становится кириллицей и наоборот. Сразу после этого ещё один двойной Shift вернёт как было.",
        );
        weak(
            ui,
            p,
            "Работает в приложениях, которые публикуют выделение (PRIMARY): Qt и KDE, GTK, Firefox, Chromium. В терминале выделение не заменяется набором. В KDE, Hyprland и Sway выделение берётся через wl-clipboard, в GNOME и X11 — через xclip.",
        );
    });
    ui.add_space(12.0);

    card(ui, p, |ui| {
        card_title(ui, p, icons::PAUSE, "Пауза — например, в играх");
        bullet(ui, p, "Переключатель на главной странице.");
        bullet(
            ui,
            p,
            "Значок в трее: щелчок колесом мыши или пункт меню «Исправлять раскладку».",
        );
        bullet(
            ui,
            p,
            "Команда shiftfix --toggle — её удобно повесить на горячую клавишу в настройках системы.",
        );
        ui.add_space(4.0);
        if widgets::button(ui, p, icons::HOUSE, "На главную", false).clicked() {
            app.page = Page::Home;
        }
    });
    ui.add_space(12.0);

    card(ui, p, |ui| {
        card_title(ui, p, icons::KEYBOARD, "Устройства");
        text(
            ui,
            p,
            "shiftfix слушает клавиатуры напрямую и следит за кликами: клик мыши, тап по тачпаду, касание экрана или пера означают, что курсор мог сместиться, и запомненное слово забывается.",
        );
        weak(
            ui,
            p,
            "Роль каждого устройства определяется автоматически. Если сенсорный экран принят за тачпад, виртуальная клавиатура (keyd, kanata) дублирует настоящую или нужная клавиатура не найдена — откройте «Устройства» и выберите роль вручную.",
        );
        weak(
            ui,
            p,
            "keyd виртуальную клавиатуру shiftfix не трогает сам. Для kanata добавьте в defcfg: linux-dev-names-exclude (\"shiftfix virtual keyboard\") — иначе замены клавиш применятся к исправлению второй раз.",
        );
        if widgets::button(ui, p, icons::KEYBOARD, "Открыть «Устройства»", false).clicked()
        {
            app.page = Page::Devices;
        }
    });
    ui.add_space(12.0);

    card(ui, p, |ui| {
        card_title(ui, p, icons::WRENCH, "Если не работает");
        step_text(
            ui,
            p,
            1,
            "Откройте «Журнал» и нажмите «Проверить систему» — проверка подскажет, чего не хватает.",
        );
        step_text(
            ui,
            p,
            2,
            "Нет клавиатур или «нет доступа к /dev/input» — вы не в группе input. После install.sh нужна перезагрузка.",
        );
        step_text(
            ui,
            p,
            3,
            "«/dev/uinput недоступен» — не применилось udev-правило из install.sh; перезагрузка помогает.",
        );
        step_text(
            ui,
            p,
            4,
            "Слово исправляется, а выделение — нет: поставьте пакеты wl-clipboard и xclip (в GNOME и X11 нужен xclip).",
        );
        step_text(
            ui,
            p,
            5,
            "Теряются буквы при исправлении — увеличьте «Задержку между нажатиями» в настройках.",
        );
        step_text(
            ui,
            p,
            6,
            "Двойной Shift срабатывает сам — уменьшите «Паузу между нажатиями».",
        );
        if widgets::button(ui, p, icons::STETHOSCOPE, "Открыть «Журнал»", false).clicked()
        {
            app.page = Page::Log;
        }
    });
    ui.add_space(12.0);

    card(ui, p, |ui| {
        card_title(ui, p, icons::WARNING, "Стоит знать");
        bullet(
            ui,
            p,
            "В IDE от JetBrains двойной Shift открывает «Search Everywhere» — отключите его там, если мешает.",
        );
        bullet(
            ui,
            p,
            "Чтобы читать клавиатуру, нужна группа input: она даёт это право любой программе пользователя. shiftfix набранное не сохраняет и не пишет в журнал — только длины слов.",
        );
        bullet(
            ui,
            p,
            "Вставка выделения проходит через буфер обмена: Klipper может запомнить её в истории. Прежнее содержимое буфера возвращается — и текст, и скопированные файлы. Если в буфере лежал пароль из менеджера паролей, он не возвращается: буфер очищается.",
        );
        bullet(
            ui,
            p,
            "GNOME: раскладка переключается через источники ввода GNOME (сочетание «Переключиться на следующий источник ввода», по умолчанию Super+Space, должно быть задано), выделение — через XWayland и xclip. Без XWayland выделение не исправляется.",
        );
        bullet(
            ui,
            p,
            "Sway: задавайте раскладки для всех клавиатур — input type:keyboard { xkb_layout us,ru }. Слово перенабирается через виртуальную клавиатуру shiftfix, и без этих раскладок она напечатает его в своей.",
        );
    });
    ui.add_space(12.0);

    card(ui, p, |ui| {
        card_title(ui, p, icons::TERMINAL_WINDOW, "Команды");
        for (cmd, what) in [
            ("shiftfix --status", "состояние службы"),
            ("shiftfix --toggle", "пауза / продолжить"),
            ("shiftfix --check", "проверить систему"),
            ("journalctl --user -u shiftfix -f", "журнал в терминале"),
            ("systemctl --user restart shiftfix", "перезапустить службу"),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(cmd).monospace().color(p.text));
                ui.label(RichText::new(format!("— {what}")).color(p.weak));
            });
        }
        ui.add_space(6.0);
        ui.label(bold("Обновление", 15.0).color(p.text));
        ui.horizontal(|ui| {
            ui.label(RichText::new("./install.sh").monospace().color(p.text));
            ui.label(RichText::new("— в папке с новой версией").color(p.weak));
        });
        weak(
            ui,
            p,
            "Соберёт, заменит программу и запустит службу снова, если она работала. Настройки и выбор автозапуска не трогает.",
        );
        ui.add_space(6.0);
        ui.label(bold("Удаление", 15.0).color(p.text));
        ui.horizontal(|ui| {
            ui.label(RichText::new("./uninstall.sh").monospace().color(p.text));
            ui.label(RichText::new("— в папке, откуда ставили (рядом с install.sh)").color(p.weak));
        });
        weak(
            ui,
            p,
            "Остановит службу и удалит программу, ярлык, автозапуск, правило udev и членство в группе input. Настройки останутся; удалить и их: ./uninstall.sh --purge. Сначала покажет список и спросит.",
        );
    });
    ui.add_space(12.0);

    let about_card = ui.scope(|ui| about(app, ui, p)).response;
    if std::mem::take(&mut app.about_focus) {
        about_card.scroll_to_me(Some(Align::TOP));
    }
}

/// «О программе»: версии окна и службы, конфиг, ссылка на проект.
fn about(app: &mut App, ui: &mut Ui, p: &Palette) {
    card(ui, p, |ui| {
        card_title(ui, p, icons::INFO, "О программе");
        let service = app.status().map(|s| (s.version.clone(), s.pid));
        Grid::new("about")
            .num_columns(2)
            .spacing(vec2(18.0, 6.0))
            .show(ui, |ui| {
                ui.label(RichText::new("Окно").color(p.weak));
                ui.label(RichText::new(format!("shiftfix-gui {VERSION}")).color(p.text));
                ui.end_row();

                ui.label(RichText::new("Служба").color(p.weak));
                match &service {
                    Some((version, pid)) => ui.label(
                        RichText::new(format!("shiftfix {version} · pid {pid}")).color(p.text),
                    ),
                    None => ui.label(RichText::new("не отвечает").color(p.weak)),
                };
                ui.end_row();

                ui.label(RichText::new("Настройки").color(p.weak));
                ui.label(
                    RichText::new(shown_path(&app.config_path()))
                        .monospace()
                        .color(p.text),
                );
                ui.end_row();

                let url = env!("CARGO_PKG_REPOSITORY");
                ui.label(RichText::new("Проект").color(p.weak));
                let shown = url.trim_start_matches("https://");
                ui.hyperlink_to(RichText::new(shown).color(p.accent), url)
                    .on_hover_text(url);
                ui.end_row();
            });
        if let Some((version, _)) = service
            && versions_differ(VERSION, &version)
        {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(widgets::icon(icons::WARNING, 16.0).color(p.warning));
                ui.label(
                    RichText::new(format!(
                        "Служба {version}, окно {VERSION} — перезапустите службу."
                    ))
                    .color(p.warning),
                );
            });
            ui.add_space(4.0);
            if widgets::button(
                ui,
                p,
                icons::ARROWS_CLOCKWISE,
                "Перезапустить службу",
                false,
            )
            .clicked()
            {
                app.service(ServiceAction::Restart);
            }
        }
    });
}
