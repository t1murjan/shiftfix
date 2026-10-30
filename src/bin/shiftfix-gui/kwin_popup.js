// KWin-скрипт попапа shiftfix (Plasma 6, Plasma 5): ставит окно попапа у
// курсора — пользователь только что щёлкнул по значку в трее, курсор у
// значка, — и держит его поверх окон, без панели задач и переключателя окон.
//
// Скрипт загружает shiftfix-gui --popup перед показом попапа и выгружает,
// когда попап закрылся (kwin.rs). Константы CLASS (класс окна попапа) и GAP
// (отступ) он дописывает в начало файла.
//
// Положение — как в popup_position (placement.rs): правьте вместе.

function place(window) {
    if (window.resourceClass !== CLASS) {
        return;
    }
    const cursor = workspace.cursorPos;
    const output = workspace.screenAt(cursor);
    const area = workspace.clientArea(KWin.PlacementArea, output, workspace.currentDesktop);
    // Курсор в нижней половине области — панель снизу, попап над курсором.
    const top = cursor.y >= area.y + area.height / 2
        ? cursor.y - GAP - window.height
        : cursor.y + GAP;
    const left = cursor.x - window.width / 2;
    // Только копия прежней геометрии: объект {x, y, width, height}, собранный
    // с нуля, KWin 5.26–6.x молча не применяет (нет преобразования в QRectF),
    // и окно остаётся посреди экрана.
    const geometry = Object.assign({}, window.frameGeometry);
    geometry.x = within(left, area.x, area.width, window.width);
    geometry.y = within(top, area.y, area.height, window.height);
    window.frameGeometry = geometry;
    window.keepAbove = true;
    window.skipTaskbar = true;
    window.skipSwitcher = true;
}

// Сдвинуть отрезок длины len, начатый в pos, внутрь start..start + span с
// отступами GAP от краёв; если не помещается — к start + GAP.
function within(pos, start, span, len) {
    return Math.max(Math.min(pos, start + span - GAP - len), start + GAP);
}

// Plasma 6 — windowAdded и windowList, Plasma 5 — clientAdded и clientList.
// Попап мог открыться и раньше, чем KWin выполнил скрипт, — тогда он уже
// среди окон.
(workspace.windowAdded || workspace.clientAdded).connect(place);
(workspace.windowList ? workspace.windowList() : workspace.clientList()).forEach(place);
