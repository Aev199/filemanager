# Atlas alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Сделать существующий интерфейс плотнее и вернуть длинную цепочку Miller.
**Architecture:** layout.rs хранит чистую геометрию/прокрутку, palette.rs — цвета;
view.rs использует их непосредственно. ScrollHandle и изменяемые ширины каждой панели независимы,
показ текущей колонки после prepaint опирается на фактические bounds.
**Tech Stack:** Rust 2024, GPUI CE 0.2.2, GPUI components 0.2.0.
**Spec:** docs/superpowers/specs/2026-10-11-atlas-alignment-design.md

## Global Constraints

- Сохранить Rust + GPUI, безопасные операции и журнал без копий содержимого.
- Сохранённые сеансы/настройки не переписывать, main не менять.
- Выполнять последовательно, без повторных подтверждений; push рабочей ветки разрешён.
- Windows screenshot/manual smoke отдельно от проверки кода; CI не запускать на каждый commit.

## Review Focus

- Глубокий путь: текущая колонка доступна, все предки остаются в цепочке.
- Панель уже 200 px: текущая папка доступна, preview не вытесняет её.
- Назад/вверх и переключение вкладок: прокрутка показывает новое текущее место.
- Ручной resize колонок: показ текущей колонки использует реальные bounds.
- Светлая тема/hidden/multiselect: белый текст и иконки читаются на selected.

### Task 1: Длинная цепочка колонок

**Files:** layout.rs, view.rs, main.rs (crates/desktop/src/).
**Interfaces:** column_widths(width,count,current) -> Vec<f32>;
reveal_offset(viewport,start,end,offset) -> f32; column_capacity(width,preview) -> usize.

- [x] Добавить failing tests: ширина 1200 вмещает >3 колонки; семь колонок
  не сжимаются меньше своих минимумов; reveal deep/right и up/left,
  already-visible сохраняет offset, oversized current показывает начало.
- [x] Run: rustc --edition=2024 --test crates/desktop/src/layout.rs -o /tmp/atlas-layout-tests;
  /tmp/atlas-layout-tests. Expected: новые тесты FAIL по геометрии/отсутствующей функции.
- [x] Убрать cap=3, изменить минимумы 144/200/160, расширять только current
  до свободной ширины; собирать все ancestors. При width<360 убрать preview.
- [x] Привязать отдельные ScrollHandle к панелям; current on_prepaint + defer
  вызывает reveal_offset по реальным bounds только при смене reveal key.
- [x] Run policy tests. Expected: все PASS. Commit task.

### Task 2: Плотность и состояния

**Files:** palette.rs, layout.rs, view.rs.
**Interfaces:** ROW_HEIGHT=24; SELECTION_TEXT; SELECTED, TRAIL, INACTIVE_SELECTED.

- [x] Добавить failing contrast tests selection text/background для обеих тем;
  различимость inactive/trail/hover. Expected: текущий TEXT на новом ярком фоне
  либо отсутствующие состояния не удовлетворяют политике.
- [x] Установить выбранный фон 0069D9 и белый текст, trail 24456B,
  inactive 35465C; light mapping соответствующий. Семантическую пару цветов
  применять к тексту, иконке и metadata selected rows, включая hidden.
- [x] Использовать ROW_HEIGHT=24 в virtual lists/header и поисковых результатах;
  убрать rounded_sm у file rows. Не менять вертикальный scroll API.
- [x] Run оба policy suites. Expected: PASS. Commit task.

### Task 3: Проверки, ревью и handoff

**Files:** docs/FINALIZATION_PLAN.md, docs/SMOKE_TEST.md,
docs/ATLAS_ALIGNMENT_2026-10-11.md, этот план.
**Interfaces:** реальные production layout/palette файлы тестируются и в ручном Windows workflow.

- [x] Run cargo test -p filemanager-core -p filemanager-cli --all-targets;
  RUST_FONTCONFIG_DLOPEN=1 cargo check --workspace --all-targets;
  git diff --check. Expected: PASS; ограничения среды записывать точно.
- [x] Независимое whole-change ревью. Исправить Important/Critical,
  не выдавать отсутствие Windows screenshot за успешную визуальную проверку.
- [x] Записать handoff, Windows сценарии deep-chain/resize/DPI/selection/transfer;
  update plan status; commit и push feat/native-gpui-foundation без force.

## Итог проверок

Full Linux cargo test --workspace PASS: 170 tests, including viewport construction.
Final review fixes: actual scroll container and independent column width growth;
both recorded RED→GREEN in progress ledger. Windows screenshot/smoke остаётся
открытым пунктом финализации, этот план не выдаёт policy tests за проверку окна.
