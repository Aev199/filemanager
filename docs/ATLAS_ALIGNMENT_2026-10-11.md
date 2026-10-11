# Интерфейс после сверки с Atlas — 11.10.2026

## Для следующего разработчика

Основание: официальный screenshot и выборки кадров шести видео Atlas,
перечисленные в docs/superpowers/specs/2026-10-11-atlas-alignment-design.md.
Работа начинается с ea629ed, продолжается в feat/native-gpui-foundation.

- Полная цепочка ancestors + optional preview сохраняется; cap=3 удалён.
  layout::column_paths используется renderer и проверяется тестом глубокого пути.
- Начальные widths 144/200/160 px (ancestor/current/preview); свободная ширина
  достаётся current. Preview убирается при viewport<360, ancestors не обрезаются.
- У каждой стороны свой ScrollHandle. column_scroll_area создаёт настоящий
  overflow_x_scroll viewport и horizontal_scrollbar с тем же handle.
  Не заменять это на overflow_x_scrollbar(): в components 0.2.0 обёртка создаёт
  отдельный внутренний viewport/handle; explicit handle попадёт в content.
- Current wrapper on_prepaint получает actual bounds. Через defer рассчитывает
  минимальный reveal current+preview (когда помещаются) и clamp по max offset.
  Навигационный ключ включает tab, side, current role, путь, preview и ширину.
  Обычные notify не перезаписывают ручную прокрутку.
- Miller widths находятся в Explorer.column_sizes отдельно для двух сторон.
  Их нельзя помещать в fixed-total h_resizable: при overflow и minimum widths
  GPUI не даст расширять колонку. resize_column меняет одну ширину, остальные
  сохраняет, увеличивает общий content. Внешние sidebar/split/inspector всё ещё
  используют штатный h_resizable. Ручные column widths временные; смена пути,
  preview, вкладки или ширины viewport возвращает расчётные значения.
- Divider 6 px управляет ColumnResize: движение относительно initial pointer,
  role minimum..4000 px; mouse up/out завершает, Esc возвращает исходные widths.
  Жест не запускает перенос файлов и не смешивается с marquee.
- layout::ROW_HEIGHT=24 используется в virtual file rows, search rows,
  column header и marquee hit testing. Не возвращать отдельные 28px константы
  в begin_marquee/move_marquee: выбор станет соответствовать другим строкам.
- SELECTED=0069D9, SELECTION_TEXT=FFFFFF; active/inactive/trail/hover различны.
  File name, metadata, selected icons/chevrons, command palette и drag preview
  используют semantic selection foreground. Hidden selected icons не затемняются.
- Копирование, история без копий содержимого, saved preferences/session не менялись.

## Проверка

- 16 layout + 4 palette standalone tests PASS, реальные production modules.
- cargo test --workspace -j 2: PASS, 170 tests (145 core, 3 CLI, 22 desktop).
- cargo check --workspace --all-targets: PASS. git diff --check: PASS.
- Linux использует RUST_FONTCONFIG_DLOPEN=1. Для тестового linker передан
  LIBRARY_PATH=/tmp/filemanager-native-link: libxcb.so, libxkbcommon.so,
  libxkbcommon-x11.so — ссылки на установленные runtime .so.1/.so.0.
  Репозиторий и системные пакеты этим не изменены; для обычного Linux dev
  подходят штатные development packages. Это не Windows-проверка.
- Независимое ревью: два Important устранены; низкая читаемость других
  SELECTED consumers переоценена как Important и тоже исправлена.
  Viewport construction test сначала FAIL (overflow None), потом PASS.
- Windows CI/EXE/screenshots после этой работы ещё не получены.
  Старый run №19 и IMG_4504.png показывают интерфейс до этих изменений.

## Обязательный Windows smoke

1. Создать временную цепочку минимум 8 вложенных папок с длинными именами.
   Ctrl+L → самый глубокий путь: current видна, предки доступны горизонтально.
2. Стрелки/Enter/Backspace/Back/Forward, mouse selection подпапки с preview,
   вкладки и оба split: current+preview видны, если помещаются.
3. Прокрутить предков вручную; дождаться background refresh: позиция не сбрасывается.
4. Изменить ширину любой колонки, включая последнюю: соседние widths прежние,
   scrollbar учитывает новую сумму. Esc во время жеста возвращает ширину.
5. Окна 760/960/1360 px, DPI 100/125/150%, узкий split, обе темы.
6. Marquee в пустой области/gutter и после vertical scroll: 24px строки
   выбираются точно. Ctrl/Shift/multiselect, скрытые файлы, F2 не теряются.
7. Выделение file/command, shortcut/date/size/icon и drag preview читаются.
8. Drop Zone copy/move/clear, внутренний и внешний drop — прежние операции.
9. Снять настоящие окна в обеих темах: список, глубокие колонки, split,
   inspector с историей; сравнить с Atlas на одинаковой ширине окна.

## Отдельный этап

Произвольное tiling на три и более области — отдельное изменение модели панелей.
Облака/FTP/плагины по исходному соглашению не добавляются.

## Финальная проверка доступной среды (11.10, после публикации)

Проверен исходный коммит `f60b89f91b37dae9b392c6b946f53630c1c34fec`.

- `cargo build -p filemanager-desktop -j2` на Linux: PASS; получен нативный
  debug executable. Использованы те же RUST_FONTCONFIG_DLOPEN/LIBRARY_PATH,
  что в проверке выше. Это подтверждает сборку, а не отрисовку окна.
- `cargo check --workspace --all-targets --target x86_64-pc-windows-msvc -j2`
  в Linux: НЕ ЗАВЕРШЁН. Native dependency build остановился на отсутствии
  MSVC `lib.exe`; это ограничение окружения, а не диагностированная ошибка
  исходников. Переносить этот результат в Windows PASS нельзя.
- GitHub Actions проверен повторно: новых запусков после №19 нет.
  Повтор №19 использовал бы старый коммит `557af78`, поэтому не запускался.
- GitHub connector умеет читать и повторять существующие runs, но не
  предоставляет workflow_dispatch для новой версии. Автоматические
  триггеры workflow ради обхода этого ограничения не добавлялись.
- Linux window capture не выполнен: Xvfb отсутствует, установка пакетов
  в текущей среде недоступна. Настоящего нового снимка интерфейса нет.

Следующий обязательный шаг: открыть
https://github.com/Aev199/filemanager/actions/workflows/windows-portable.yml,
выбрать **Run workflow → feat/native-gpui-foundation** и запустить один раз.
Если после этой записи добавлен только документационный коммит, он также
подходит: исходники приложения совпадают с проверенным `f60b89f`.
Проверить head SHA запуска, все тестовые шаги, ZIP и screenshot artifact;
скриншот — best effort и его отсутствие не означает успех визуальной проверки.
Затем выполнить девять пунктов Windows smoke выше. До этого не отмечать
интерфейс как проверенный на Windows и не считать EXE из №19 актуальным.
