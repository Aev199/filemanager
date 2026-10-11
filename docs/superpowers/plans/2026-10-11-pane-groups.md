# План: панели со своими вкладками, как в Atlas

Исполнитель — более дешёвая модель. План самодостаточен: в нём всё, что
нужно знать о коде, порядок шагов, проверки и запреты. Шаги делать строго по
порядку; каждый шаг — отдельный коммит с зелёными проверками (раздел 9).

## 1. Зачем и что получит пользователь

Пользователь держит 10–15 папок одновременно, в том числе с разных дисков.
Сейчас окно = список вкладок, вкладка = одна или две половины. В Atlas наоборот:
окно делится на **панели**, у каждой панели **свои вкладки** и своя строка
навигации. Нужно:

- Окно делится на 1–4 панели, по горизонтали и вертикали, произвольно
  (дерево). Разделители перетаскиваются мышью.
- У каждой панели: строка вкладок, строка навигации (← → ↑ и путь), содержимое
  (список или колонки). Активная панель обведена тонкой синей линией (ACCENT).
- Вкладку можно перетащить: на строку вкладок другой панели — вкладка
  переезжает туда; на содержимое панели — появляется полупрозрачная синяя
  подсветка зоны (левая/правая/верхняя/нижняя половина или центр); отпускание
  делит панель в эту сторону или переносит вкладку в центр.
- Много вкладок (15+) в одной панели остаются удобными: строка вкладок
  прокручивается колесом мыши, активная вкладка всегда видна, кнопка «⌄» в
  конце строки открывает список всех вкладок панели, Ctrl+Shift+T открывает
  последнюю закрытую вкладку, подсказка на вкладке — полный путь.
- Сеанс и рабочие пространства сохраняют раскладку, панели и вкладки. Старые
  сохранения открываются без потерь.

Не входит: панели в отдельных окнах, синхронная прокрутка, перенос панелей
между окнами.

## 2. Обязательные правила (не нарушать)

- Ветка `feat/native-gpui-foundation`. В `main` не вливать, PR не создавать,
  пока пользователь не попросит.
- Интерфейс только на русском. Подписи короткие, без пояснительного «шума»
  (см. раздел «слоп» в `docs/ATLAS_REVIEW_2026-10-11.md`).
- Никогда не читать и не хранить содержимое файлов пользователя; в SQLite
  только пути и метаданные.
- Файловые операции не менять. Копирование/перенос/Undo/Корзина работают
  через существующие функции; новые панели лишь выбирают пути.
- Windows-сборка `windows-portable.yml` запускается только вручную
  (workflow_dispatch), один раз в конце этапа, не на каждый коммит.
- Сообщение коммита заканчивается строками
  `Co-Authored-By: …` и `Claude-Session: …` (как в предыдущих коммитах ветки,
  см. `git log -1`). В коммитах и коде не упоминать названия моделей.
- Не переформатировать файлы целиком (`cargo fmt` не запускать — репозиторий
  не отформатирован rustfmt, дифф станет нечитаемым).

## 3. Как устроено сейчас (факты из кода на d6d4258)

Ядро, `crates/core/src/browser.rs`:

```rust
pub struct Pane { pub path: PathBuf, back: Vec<PathBuf>, forward: Vec<PathBuf> } // история навигации
pub struct Tab { pub pinned: bool, pub title: String, pub left: Pane, pub right: Option<Pane>, pub focus_right: bool }
pub struct Browser { pub tabs: Vec<Tab>, pub active_tab: usize }
// Browser: new, active, active_mut, new_tab, move_tab, toggle_pin, close_other_tabs, close_tab
// Tab: new, active, active_mut, toggle_split, navigate
```

Сохранение, `crates/core/src/workspace.rs`: таблица
`workspace_tabs(workspace_name, position, left_path, right_path, focus_right, pinned)`;
`save(name, &Browser, miller_mode)`, `load(name) -> Option<(Browser, bool)>`.
Сеанс — рабочее пространство с именем `SESSION_WORKSPACE = "Default"`
(`crates/desktop/src/main.rs:123`). Пропавшие папки при загрузке пропускаются.

Интерфейс, `crates/desktop/src`:

- `main.rs:41` — `enum Side { Left, Right }`; это «левая/правая половина
  активной вкладки». Около 40 мест используют `Side`.
- `main.rs:196–199` — массивы на две половины:
  `pane_measured_widths: [Option<f32>; 2]`, `column_scroll_handles: [ScrollHandle; 2]`,
  `column_reveal_keys: [Option<String>; 2]`, `column_sizes: [Option<(String, Vec<f32>)>; 2]`,
  плюс `ColumnResize.side_index`.
- `keys.rs:80` `active_side()`, `keys.rs:85` `pane_path(side)`,
  `keys.rs:126` `navigate_side(side, folder, select, cx)`,
  `keys.rs:323` `switch_tab`, `keys.rs:336` `close_tab_at`, `keys.rs:344` `toggle_split`,
  `keys.rs:501` `key_switch_pane` (Tab — переключить половину).
- `main.rs` `go_to`, `add_tab`, `open_in_new_tab`, `open_in_other_pane`,
  `close_other_tabs`, `save_workspace`/`restore_workspace` (≈1479–1541),
  `visible_directories()` → `load_visible_directories()` (≈1722): какие папки
  загружать и наблюдать.
- `view.rs:167` `tab_strip()` — одна строка вкладок в заголовке окна
  (`title_bar()`), перетаскивание вкладок `TabDrag { index, title }`.
- `view.rs` `toolbar()` — строка навигации (← → ↑ + `breadcrumbs()`),
  `pane(side)` — содержимое половины (список или колонки), в разделённом виде
  у половины заголовок с путём и синей полосой сверху у активной.
- `view.rs` `render()` (≈1600): боковая панель 220 px слева, справа колонка:
  `title_bar` → `toolbar` → `h_resizable(панели + инспектор)` → `status_bar`.
- `interface.rs:530`, `view.rs:615` — щелчок по половине делает её активной.
- `layout.rs` — чистая геометрия колонок (тестируется `rustc --test`).
- Клавиши — `keys.rs` (`KeyBinding::new(...)`), команды палитры —
  `crates/core/src/interface.rs` `commands()` (у команды есть `group`).

GPUI: `gpui_component::resizable::{h_resizable, v_resizable, resizable_panel}`;
у группы есть `on_resize`, у панели `size`, `size_range`. Размеры группы
хранятся по её id: **id должен меняться при изменении структуры дерева**,
иначе GPUI применит старые размеры к новым панелям (см. `column_identity` в
`layout.rs` — тот же приём).

## 4. Целевая модель (ядро)

Новый файл `crates/core/src/panes.rs` (+ `pub mod panes;` в `lib.rs`).
`browser.rs` остаётся для `Pane`, `Entry`, `Listing`, сканирования.

```rust
/// Stable across splits/closes; never reused within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupId(pub u64);

#[derive(Clone, Debug)]
pub struct Tab { pub pinned: bool, pub title: String, pub pane: Pane }   // одна папка + история

#[derive(Clone, Debug)]
pub struct Group { pub id: GroupId, pub tabs: Vec<Tab>, pub active_tab: usize }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis { Horizontal /* бок о бок */, Vertical /* одна под другой */ }

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf(GroupId),
    Split { axis: Axis, ratio: f32 /* 0.15..=0.85, доля первой части */, first: Box<Node>, second: Box<Node> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge { Left, Right, Top, Bottom }

pub const MAX_GROUPS: usize = 4;
pub const MAX_TABS: usize = 100;          // как лимит сохранения сейчас
pub const CLOSED_TABS_REMEMBERED: usize = 20;

pub struct Workspace {                    // заменяет Browser
    pub groups: Vec<Group>,               // порядок не важен; ищется по id
    pub root: Node,
    pub active: GroupId,
    next_id: u64,
    closed: Vec<(GroupId, PathBuf)>,      // для Ctrl+Shift+T, последние сверху
}
```

API `Workspace` (каждый метод — юнит-тест в `panes.rs`):

| Метод | Поведение |
|---|---|
| `new(path)` | одна группа, одна вкладка |
| `group(id) / group_mut(id)` | `Option<&Group>` |
| `active_group() / active_group_mut()` | группа `active`, всегда существует |
| `active_tab() / active_pane() / active_pane_mut()` | вкладка/панель активной группы |
| `focus(id)` | сделать группу активной, если существует |
| `new_tab(group, path)` | вставить после активной вкладки группы, сделать активной; при `MAX_TABS` — ошибка |
| `close_tab(group, index)` | закреплённую не закрывать; закрытую запомнить в `closed`; если вкладка последняя в группе и групп > 1 — закрыть группу (`close_group`); последнюю вкладку последней группы не закрывать |
| `close_other_tabs(group, keep)` | как сейчас, внутри группы, закреплённые остаются |
| `reopen_closed()` | вернуть последнюю закрытую в её группу (или в активную, если группы нет); пропавшую папку пропустить и взять следующую |
| `move_tab_within(group, from, to)` | как нынешний `Browser::move_tab` |
| `move_tab(from_group, index, to_group, to_index)` | перенести между группами; опустевшая группа закрывается; закреплённость сохраняется |
| `split(group, edge, tab: Option<(GroupId, usize)>)` | новая группа рядом с `group` с края `edge`; если `tab` задан — перенести эту вкладку туда, иначе — копию активной вкладки `group` (тот же путь, пустая история); при `MAX_GROUPS` — ошибка; новая группа активна |
| `close_group(id)` | убрать лист, родитель заменить вторым ребёнком; активной сделать соседнюю группу; последнюю группу не закрывать |
| `set_ratio(path_to_split: &[bool], ratio)` | путь в дереве (false = first, true = second); ratio обрезать до 0.15..=0.85 |
| `neighbour(id, edge) -> Option<GroupId>` | ближайшая группа с этой стороны (для «в другую панель» и Alt+стрелок) |
| `other_group(id) -> Option<GroupId>` | для «Открыть в другой панели»: последняя активная группа, кроме `id`; если групп одна — `None` |
| `leaves() -> Vec<GroupId>` | в порядке обхода (слева направо, сверху вниз) |
| `signature() -> String` | строка структуры дерева без ratio, например `h(1,v(2,3))` — для id resizable-групп |

Инварианты (проверять `debug_assert!` в одном `fn check(&self)` и в тестах):
каждая группа непуста; каждый `GroupId` дерева есть в `groups` и наоборот;
`active` существует; `active_tab < tabs.len()`; групп 1..=MAX_GROUPS.

Удалить из ядра `Tab.left/right/focus_right`, `Tab::toggle_split`, старый
`Browser`; `workspace.rs` перевести на `Workspace` (`cli` модель вкладок не
использует).

## 5. Хранение (SQLite, `workspace.rs`)

Миграция в `WorkspaceStore::open` в транзакции `BEGIN IMMEDIATE` (как сделана
миграция `pinned`):

- `ALTER TABLE workspace_tabs ADD COLUMN group_id INTEGER NOT NULL DEFAULT 0`
  (если колонки нет).
- `CREATE TABLE IF NOT EXISTS workspace_groups (workspace_name TEXT NOT NULL,
  group_id INTEGER NOT NULL, active_tab INTEGER NOT NULL,
  PRIMARY KEY(workspace_name, group_id))`.
- `CREATE TABLE IF NOT EXISTS workspace_layout (workspace_name TEXT PRIMARY KEY,
  tree TEXT NOT NULL, active_group INTEGER NOT NULL)`.

Формат `tree` — простая строка без serde:
`leaf := число`, `node := ("h"|"v") "(" ratio "," node "," node ")"`,
пример `h(0.5,1,v(0.6,2,3))`. Функции `Node::encode()` и
`Node::decode(&str) -> Option<Node>` с тестами: круговой проход, мусор →
`None`, глубина больше 8 → `None`, ratio вне 0..1 → обрезка.

Сохранение (`save`): удалить строки рабочего пространства во всех трёх
таблицах, записать группы, вкладки (`position` внутри группы, `group_id`,
`left_path` = путь вкладки, `right_path` = NULL), дерево. Ограничения:
вкладок всего ≤ 100, групп ≤ 4.

Загрузка (`load`):

1. Если для имени есть строка в `workspace_layout` — новый формат: собрать
   группы по `group_id`, порядок по `position`; вкладки с пропавшими папками
   пропустить; пустые группы убрать из дерева (как `close_group`); если групп
   не осталось — вернуть `None`.
2. Иначе — **старый формат**: все вкладки в одну группу (порядок, закрепление
   как были). Если у какой-то старой вкладки был `right_path`, создать вторую
   группу справа (`Axis::Horizontal`, ratio 0.5) с одной вкладкой —
   `right_path` **активной** старой вкладки (а если у активной его не было —
   первой вкладки, у которой он был). Остальные `right_path` не терять:
   добавить их вкладками во вторую группу, без повторов.
3. `miller_mode` как сейчас.

Тесты (`workspace.rs`, `mod tests`): круговой проход 3 группы / 7 вкладок /
закреплённые; старая база с `right_path` открывается как две группы; пропавшая
папка и пустая группа; повреждённое дерево → новый формат игнорируется,
вкладки открываются одной группой; повторное `open` не ломает схему.

## 6. Интерфейс (desktop)

### 6.1 Замена `Side` на `GroupId` (механический шаг, без нового вида)

- `main.rs`: `struct Explorer { browser: Browser … }` → `workspace: Workspace`
  (имя поля можно оставить `browser`, чтобы дифф был меньше — тогда тип
  `Workspace`).
- Удалить `enum Side`. Везде, где был `side: Side`, — `group: GroupId`.
  `active_side()` → `self.browser.active`. `pane_path(group)` →
  `self.browser.group(group).map(|g| g.tabs[g.active_tab].pane.path.clone())`.
- Массивы `[_; 2]` → `HashMap<GroupId, _>` (`pane_measured_widths`,
  `column_scroll_handles`, `column_reveal_keys`, `column_sizes`);
  `ColumnResize.side_index` → `group: GroupId`. Удалять записи закрытых групп
  при `close_group`.
- `MenuTarget::Tab(usize)` (`main.rs:127`) → `Tab(GroupId, usize)`; разбор
  в `view.rs` (`MenuTarget::Tab(index) =>`).
- Выделение рамкой: поле `side: crate::Side` (`interface.rs:470`) → `GroupId`.
- `scroll_handles: HashMap<PathBuf, _>` (по папке) сделать
  `HashMap<(GroupId, PathBuf), _>`, иначе две панели с одной папкой будут
  прокручиваться вместе.
- `visible_directories()` — объединение колонок **всех** групп (активная
  вкладка каждой группы), без повторов. Это же определяет наблюдение за
  папками.
- Фильтр (`self.filter`) действует только на активную группу — как сейчас
  на активную половину (`folder_view`). Не менять.
- `open_in_other_pane(path)`: `other_group(active)`; если нет — `split(active,
  Edge::Right, None)` и навигация в новой группе.
- Копирование/перенос «в другую панель» (найти по `open_in_other_pane`/
  `other_side`/`Side::Right` в `main.rs` и `view.rs`): цель — `other_group`.

После 6.1 приложение должно выглядеть и работать как раньше (одна группа,
Ctrl+\ даёт две группы бок о бок). Коммит.

### 6.2 Отрисовка дерева и вкладки в каждой панели

- `title_bar()`: убрать из него `tab_strip`. Оставить: кнопку боковой панели
  (когда скрыта), пустую зону перетаскивания окна, кнопку панели сведений.
  Высоту уменьшить до 32 px. (Отличие от Atlas — там вкладки и есть заголовок;
  допустимо, отметить в ATLAS_REVIEW.)
- Удалить общий `toolbar()` из `render()`.
- Новая `fn group_view(&mut self, id: GroupId, cx) -> AnyElement`:
  колонка `tab_strip(id)` (h 34) → `nav_row(id)` (h 36, ← → ↑ + путь, бывший
  `toolbar`, но для группы `id`) → `pane(id)` (бывший `pane(side)` без
  старого заголовка половины). У активной группы — рамка 1 px ACCENT по
  периметру (`border_1`), у остальных — `border_1` цвета BORDER, чтобы
  содержимое не прыгало. Щелчок мышью в любом месте группы
  (`on_mouse_down` с `capture`/обычный) делает её активной, если не активна.
- Новая `fn layout_view(&mut self, node: &Node, path: Vec<bool>, cx) -> AnyElement`:
  `Leaf(id)` → `group_view(id)`; `Split` → `h_resizable`/`v_resizable` с id
  `format!("split-{}-{}", signature, path_string)`, две `resizable_panel()`
  с `size(px(ratio * доступный_размер))`, `size_range(px(240.)..px(10000.))`,
  `on_resize` → `self.browser.set_ratio(&path, новая_доля)`. Доступный размер
  взять из `window.viewport_size()` минус боковая панель и инспектор (как
  сейчас считается `pane_width`); для вложенных узлов умножать на долю
  родителя. Точность до пикселя не нужна — GPUI сам растянет.
- В `render()`: `panels = h_resizable(main…)` с двумя детьми:
  `layout_view(root)` и инспектор (если включён). Остальное без изменений.
- `pane_width_for(group)` использует измеренную ширину группы
  (`pane_measured_widths[group]`, заполняется в `on_prepaint`, как сейчас).

### 6.3 Строка вкладок группы (много вкладок)

`tab_strip(id)`:

- Вкладка: иконка папки (FOLDER), имя, точка для закреплённой, крестик при
  наведении/у активной. Ширина `min_w(96) max_w(200)`, имя `truncate()`.
  Подсказка (`tooltip`) — полный путь (`path.display()`), чтобы различать
  `D:\Проект\Документы` и `C:\Users\…\Документы`.
- Если в одной группе несколько вкладок с одинаковым именем — показывать имя
  родителя через « · »: `Документы · Проект`. Чистая функция
  `tab_titles(&[PathBuf]) -> Vec<String>` в `crates/core/src/panes.rs` с
  тестами.
- Контейнер вкладок: `overflow_x_scroll()` с `ScrollHandle` на группу
  (`tab_scroll: HashMap<GroupId, ScrollHandle>`); при смене активной вкладки —
  `scroll_to_item(active_tab)`. Колесо мыши прокручивает по горизонтали
  (`on_scroll_wheel`: вертикальную дельту применить к оси x).
- После вкладок: «+» (новая вкладка в этой группе), затем кнопка «⌄» —
  меню со всеми вкладками группы (имя + путь серым), щелчок — переключиться.
  Использовать существующий механизм контекстного меню (`open_menu`,
  `MenuTarget` — добавить вариант `TabList(GroupId)`).
- Средняя кнопка — закрыть; двойной щелчок по пустому месту строки — новая
  вкладка; правая кнопка по вкладке — существующее меню вкладки
  (закрепить, закрыть другие) + новые пункты «Переместить в другую панель»,
  «Разделить вправо», «Разделить вниз».
- Перетаскивание: `TabDrag { group: GroupId, index: usize, title }`. Drop на
  вкладку своей группы — `move_tab_within`; на вкладку или пустое место
  строки другой группы — `move_tab(…, to_index)`.

### 6.4 Перетаскивание вкладки на содержимое (док-зоны)

- Чистая функция в `crates/desktop/src/layout.rs`:
  `pub fn dock_zone(x: f32, y: f32, w: f32, h: f32) -> DockZone`, где
  `enum DockZone { Left, Right, Top, Bottom, Center }`: крайние 30 % ширины —
  Left/Right, крайние 30 % высоты — Top/Bottom (при попадании в угол — по
  ближайшему краю), иначе Center. Тесты: центр, четыре края, углы,
  нулевой размер → Center.
- `pane(id)` получает `.drag_over::<TabDrag>` (стиль не менять) и
  `on_drag_move::<TabDrag>`: запоминать `self.dock_hover = Some((id, zone))`
  по позиции мыши относительно `bounds` группы (bounds сохранять в
  `on_prepaint`). Поверх группы при `dock_hover == Some((id, _))` рисовать
  абсолютный прямоугольник нужной половины/всей области:
  `bg(rgba(0x5E8BFF22))`, `border_2`, ACCENT, скругление 6 px. Без анимации
  (Atlas показывает подсветку мгновенно).
- Drop: Center → `move_tab(src, idx, id, конец)`; край → `split(id, edge,
  Some((src, idx)))`. Если групп уже 4 — край работает как Center, статус
  «Не больше четырёх панелей». Перетаскивание единственной вкладки группы на
  край этой же группы — ничего не делать.
- Сбрасывать `dock_hover` на `on_mouse_up`, `on_mouse_up_out`, Esc и при
  уходе курсора (`on_drag_move` в другую группу перезаписывает).

### 6.5 Клавиши и команды

В `keys.rs` (контекст `Filemanager`) и в `commands()` (группа
«Вкладки и панели»):

| Действие | Клавиша | Примечание |
|---|---|---|
| Разделить вправо | Ctrl+Shift+S | как в Atlas; **«Для переноса» (Stage) перенести на Ctrl+Shift+D** |
| Разделить вниз | Ctrl+Alt+S | как в Atlas |
| Закрыть панель | Ctrl+Shift+W | последнюю не закрывать |
| Разделить окно (переключить) | Ctrl+\\ | оставить: нет групп → вправо; есть → закрыть активную |
| Следующая / предыдущая панель | Tab / Shift+Tab | вместо `SwitchPane`, по `leaves()` |
| Панель слева/справа/сверху/снизу | Alt+Shift+стрелки | `neighbour()` |
| Следующая / предыдущая вкладка панели | Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+PgDn, Ctrl+PgUp | внутри активной группы |
| Открыть закрытую вкладку | Ctrl+Shift+T | `reopen_closed()` |
| Новая / закрыть вкладку | Ctrl+T / Ctrl+W | в активной группе |

Обновить подсказки в `palette`-командах (`shortcut`) и в
`docs/SMOKE_TEST.md`. Тест `crates/core/tests/interface_policy.rs` — добавить
проверку, что у новых команд есть группа и клавиша.

### 6.6 Мелочи, которые легко забыть

- Панель сведений и строка состояния показывают выбор **активной** группы.
- Выделение рамкой (`interface.rs`, marquee) хранит `side` — заменить на
  `GroupId`; рамка не переходит между группами.
- Переименование, новая папка, массовое переименование работают в папке
  активной группы.
- Боковая панель: щелчок по месту — навигация в активной группе;
  средний щелчок по месту — новая вкладка в активной группе (добавить).
- `Ctrl+L` / щелчок по пути редактирует путь своей группы
  (`address_editing: Option<GroupId>` вместо `bool`).
- Рабочие пространства в боковой панели: «Сохранить вкладки» сохраняет всю
  раскладку; восстановление заменяет раскладку целиком.
- Ширина окна 760 px: две панели бок о бок по 240 px минимум (+ боковая
  220) не помещаются с инспектором — `split` при недостатке места делит
  вниз вместо вправо (проверять по `viewport_size`).

## 7. Порядок и коммиты

| Шаг | Содержание | Проверка перед коммитом |
|---|---|---|
| 1 | `panes.rs`: модель, API, `encode/decode`, `tab_titles`, тесты | `cargo test -p filemanager-core` |
| 2 | `workspace.rs` на `Workspace`, миграция, старый формат, тесты | `cargo test -p filemanager-core -p filemanager-cli` |
| 3 | 6.1: desktop компилируется на новой модели, вид прежний | полный набор раздела 9 + снимок: одна панель и Ctrl+\\ |
| 4 | 6.2 + 6.3: дерево, вкладки в панелях, много вкладок | раздел 9 + снимки: 1, 2 бок о бок, 3 (h + v), 15 вкладок в панели, меню «⌄» |
| 5 | 6.4: док-зоны | раздел 9 + тесты `dock_zone` + снимок подсветки (см. 9.3) |
| 6 | 6.5 + 6.6: клавиши, команды, мелочи | раздел 9 + снимки палитры и узкого окна 760 px |
| 7 | Документы и Windows CI | см. раздел 10 |

## 8. Подводные камни

- **Id resizable-групп.** Без `signature()` в id после закрытия панели GPUI
  применит старые размеры к другим панелям (была такая ошибка с колонками —
  см. `column_identity`). Ключ: `split-<signature>-<путь в дереве>`.
- **Фокус клавиатуры.** Все действия клавиш уже вешаются на корневой
  `div` с `key_context("Filemanager")` в `render()`; не добавлять
  `key_context` на группы, иначе стрелки перестанут работать.
- **Перетаскивание вкладки и файлов.** `FileDragInfo` (файлы) и `TabDrag`
  (вкладки) — разные типы; док-зоны реагируют только на `TabDrag`. Не ломать
  drop файлов на папки и в «Файлы для переноса».
- **Одинаковые папки в разных панелях** — допустимы; кэш листингов общий
  (`directory_cache` по пути) — это хорошо, не дублировать.
- **Закрытие группы во время операции копирования.** Операции хранят пути, не
  группы, — ничего не делать, только не держать `GroupId` в фоновых задачах.
- **Xvfb.** Первый ввод после запуска может потеряться — сначала щелчок в окно.
  Кадр запаздывает за вводом: делать `move` перед `shot` (скрипт это делает).
  `xdotool type` не печатает кириллицу.

## 9. Проверки на каждом шаге

```bash
cd /home/user/filemanager   # или корень клона
cargo build -p filemanager-desktop
cargo test --workspace                                   # всё зелёное
cargo check --workspace --all-targets --target x86_64-pc-windows-gnu   # без ошибок и предупреждений
rustc --edition=2024 --test crates/desktop/src/layout.rs -o /tmp/t_layout && /tmp/t_layout
rustc --edition=2024 --test crates/desktop/src/palette.rs -o /tmp/t_palette && /tmp/t_palette
```

### 9.3 Снимки окна (Linux, Xvfb)

```bash
tools/ui-smoke/run.sh 95            # запускает target/debug/filemanager-desktop
sleep 7
tools/ui-smoke/ui.sh 95 /tmp/shots click 800 600 wait 1 key ctrl+shift+s wait 1 shot split
```

Снимки смотреть глазами (открыть PNG). Проверять: нет обрезанных подписей,
активная панель обведена, вкладки не налезают, подсветка зоны видна.
Перетаскивание вкладки для снимка док-зоны:
`xdotool mousedown 1`, несколько `mousemove`, `shot`, `xdotool mouseup 1`
(координаты — относительно окна, как в `ui.sh`).

Если тест `directory_watch` упал один раз — перезапустить один раз; если
снова — это ошибка, разбираться.

## 10. Завершение этапа

1. `docs/SMOKE_TEST.md` — раздел «Панели»: разделить вправо/вниз, перетащить
   вкладку в зону, 15 вкладок + «⌄», Ctrl+Shift+T, перезапуск сохраняет
   раскладку, старый сеанс с разделённой вкладкой открывается двумя панелями.
2. `docs/ATLAS_REVIEW_2026-10-11.md` — строку «Разделение окна» перевести в
   «совпадает», указать отличие с заголовком окна (6.2).
3. `docs/HANDOFF_2026-10-10.md`, `docs/MILESTONES.md` — короткая запись.
4. Push, затем один запуск Windows CI:
   GitHub MCP `actions_run_trigger` (`run_workflow`, `windows-portable.yml`,
   ref `feat/native-gpui-foundation`); дождаться, скачать артефакты
   `Filemanager-Windows-x64` и `Filemanager-UI-Screenshot`, посмотреть снимок.
   Красный CI — чинить по логу (`get_job_logs`), не перезапускать вслепую.
5. Отчёт пользователю на русском: что сделано, снимки, SHA-256 EXE, что
   проверить руками на Windows.
