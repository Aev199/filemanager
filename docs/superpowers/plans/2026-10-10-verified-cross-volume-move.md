# Verified cross-volume Move Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans. Execution is authorized without additional confirmations.

**Goal:** Безопасный перенос между дисками через проверенную копию и Корзину исходника.

**Architecture:** Существующая очередь остаётся границей исполнения. Новый verifier
сравнивает потоки вне UI; межтомовой Move сохраняет фазы в SQLite и отдельное
разделяемое состояние Undo в квитанции.

**Tech Stack:** Rust stable, GPUI CE, windows-sys 0.61, SQLite/rusqlite, tempfile, notify.

**Spec:** `docs/superpowers/specs/2026-10-10-verified-cross-volume-move-design.md`

## Global Constraints

- Windows-файловый менеджер; старое содержимое/хеши не сохраняются.
- Никаких перезаписей занятой цели, прямого permanent delete или auto replay.
- Background executor для файлов/SQLite; чтение кусками по 256 КБ.
- Rename не включает межтомовой fallback; только Move с Windows error 17.
- Push разрешён, main не сливаем; EXE не собираем на каждый коммит.

## Review Focus

- Полная цель создана, но проверка/Recycle/SQLite отказали: обе копии сохраняются и показаны пути.
- Дополнительные потоки NTFS, включая каталоги: потеря потока не удаляет исходник.
- Изменение исходника/цели между фазами: снимки блокируют очистку.
- Undo восстановил источник, но не убрал копию: повтор не восстанавливает второй раз.
- Другая программа создаёт исходное имя при Undo: никакой перезаписи, реальный путь виден.

### Task 1: Streaming verifier

**Files:** Create `crates/core/src/copy_verification.rs`; modify `crates/core/src/lib.rs`.

**Interfaces:** `verify(source: &Path, target: &Path, control: &CopyControl) -> io::Result<()>`.
Windows перечисляет `$DATA` через FindFirstStreamW/FindNextStreamW/FindClose;
Unix сравнивает обычное содержимое. Дополнительных зависимостей нет.

- [x] Тесты: одинаковое дерево проходит; иной байт при той же длине, новый/пропавший
  файл, ссылка и отмена отказывают; Windows ADS различие отказывает.
- [x] Запустить тесты RED, реализовать verifier, проверить GREEN.
- [x] `cargo test -p filemanager-core -p filemanager-cli --all-targets`, commit.

### Task 2: Audited cross-volume Move and Undo

**Files:** Create `cross_volume_move.rs`; modify `operations.rs`, `operation_journal.rs`.

**Interfaces:** `OperationJournal::checkpoint(id: i64, phase: &str) -> io::Result<()>`;
`InterruptedAction.phase: Option<String>`; внутренний Move использует `Receipt`
Copy и `Arc` разделяемого состояния восстановления. `Receipt::can_undo` учитывает
точную квитанцию межтомового удаления. Старые публичные Plan/queue API сохранены.

- [x] Тесты RED: сохранение исходника при отмене/конфликте, успешный файл/Undo,
  изменённая копия блокирует Undo, старый SQLite получает phase.
- [x] Реализовать checkpoint до Copy/verify/recycle и Undo restore/recycle.
- [x] Проверить error-17-only fallback и совместимость same-volume Rename/Move.
- [x] Тесты частичного Undo: source restored stage survives failure and retry;
  checkpoint refusal before source cleanup preserves source and completed copy.
- [x] Прогнать весь core/CLI, Windows all-targets cross-check, commit.

### Task 3: Desktop transfer controls and documentation

**Files:** Modify desktop `main.rs`/`view.rs`, `MILESTONES.md`, `SMOKE_TEST.md`, developer handoff.

**Interfaces:** `CopyControl::phase() -> TransferPhase`; фаза только атомарная,
status heartbeat сверяет свой Arc control, чтобы не пережить свою операцию.

- [x] Подключить Copy/Move к отмене и фазе «проверка копии»; показать диагностическую фазу.
- [x] Обновить smoke-test двумя Windows-томами, ADS, отменой и частичным Undo.
- [x] Проверить весь Windows workspace и отсутствие новых UI disk reads.

### Task 4: Final review and push

- [x] Независимое ревью диапазона изменений; исправить важные находки с RED→GREEN.
- [x] Финальные core/CLI tests, Windows all-targets check и git diff --check.
- [x] Push в рабочую ветку и проверить remote tree/head; обновить статусы плана.
- [x] Новый EXE обозначить как требующий общей ручной Windows-сборки.


## Итог выполнения

107 core + 3 CLI теста проходят; Windows all-targets MSVC check проходит.
Исправлены найденные при проверках: trailing separator для одиночного файла,
защита исчезнувшего восстановленного источника и фактический путь в частичном
Undo. Failed-задания с фазой сохраняются в диагностике. Физический межтомовой
Windows smoke и новая сборка EXE ещё не выполнены; см. общий пакет 6.
