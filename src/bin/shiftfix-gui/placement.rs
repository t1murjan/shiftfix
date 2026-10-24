//! Где поставить попап у значка в трее.

/// Отступ попапа от курсора и от краёв рабочей области: от панели и края
/// экрана.
pub const GAP: i32 = 8;

/// Левый верхний угол попапа размера `size` (ширина, высота).
///
/// `cursor` — курсор или точка щелчка по значку, `area` — рабочая область
/// монитора под ним без панелей: x, y, ширина, высота.
///
/// Курсор в нижней половине области — панель снизу, и попап встаёт над ним;
/// в верхней — под ним. По горизонтали — по центру курсора. Из области попап
/// не выходит; если он в неё не помещается — от её левого верхнего угла.
pub fn popup_position(cursor: (i32, i32), area: [i32; 4], size: (i32, i32)) -> (i32, i32) {
    let (cursor_x, cursor_y) = cursor;
    let [x, y, width, height] = area;
    let (popup_width, popup_height) = size;
    let top = if cursor_y >= y + height / 2 {
        cursor_y - GAP - popup_height
    } else {
        cursor_y + GAP
    };
    let left = cursor_x - popup_width / 2;
    (
        within(left, x, width, popup_width),
        within(top, y, height, popup_height),
    )
}

/// Сдвинуть отрезок длины `len`, начатый в `pos`, внутрь `start..start + span`
/// с отступами `GAP` от краёв; если не помещается — к `start + GAP`.
fn within(pos: i32, start: i32, span: i32, len: i32) -> i32 {
    pos.min(start + span - GAP - len).max(start + GAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POPUP: (i32, i32) = (380, 460);

    #[test]
    fn panel_at_the_bottom() {
        // Панель 48 пикселей внизу, значок справа: над курсором, у правого
        // края и над панелью с отступом.
        let area = [0, 0, 1920, 1032];
        assert_eq!(popup_position((1850, 1056), area, POPUP), (1532, 564));
        // Курсор выше панели (`shiftfix-gui --popup` из терминала): над ним.
        assert_eq!(popup_position((960, 900), area, POPUP), (770, 432));
    }

    #[test]
    fn panel_at_the_top() {
        // Waybar 32 пикселя вверху, трей справа, как в Omarchy: под курсором.
        let area = [0, 32, 1920, 1048];
        assert_eq!(popup_position((1850, 16), area, POPUP), (1532, 40));
        // Значок посередине панели: по центру курсора.
        assert_eq!(popup_position((960, 16), area, POPUP), (770, 40));
        // Значок у левого края: не левее области.
        assert_eq!(popup_position((10, 16), area, POPUP), (8, 40));
    }

    #[test]
    fn panel_on_the_right() {
        // Вертикальная панель справа, значок внизу: левее панели, над курсором.
        let area = [0, 0, 1872, 1080];
        assert_eq!(popup_position((1896, 1000), area, POPUP), (1484, 532));
        // Значок вверху панели: под курсором.
        assert_eq!(popup_position((1896, 100), area, POPUP), (1484, 108));
    }

    #[test]
    fn several_monitors() {
        // Левый монитор — с отрицательными координатами, панель внизу.
        let left = [-1920, 0, 1920, 1032];
        assert_eq!(popup_position((-70, 1056), left, POPUP), (-388, 564));
        assert_eq!(popup_position((-1900, 1056), left, POPUP), (-1912, 564));
        // Правый — выше основного, с отрицательным y, панель 32 пикселя сверху.
        let right = [1920, -368, 2560, 1408];
        assert_eq!(popup_position((4400, -384), right, POPUP), (4092, -360));
    }

    #[test]
    fn area_smaller_than_popup() {
        // Не помещается: от левого верхнего угла области.
        assert_eq!(popup_position((100, 250), [0, 0, 300, 300], POPUP), (8, 8));
        assert_eq!(
            popup_position((100, 250), [-300, 20, 300, 300], POPUP),
            (-292, 28)
        );
    }
}
