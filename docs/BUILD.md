# Build / test Filemanager on Windows

## Recommended path: GitHub Actions, manual only

The workflow is `.github/workflows/windows-portable.yml`. It has only
`workflow_dispatch`: commits and pull requests do **not** run the Windows
build. This avoids burning limited free GitHub Actions minutes.

After this workflow exists on the repository's default branch:

1. Open **GitHub → Aev199/filemanager → Actions**.
2. Pick **Windows portable EXE (manual)**.
3. Click **Run workflow**, choose the branch, and confirm.
4. When green, open the run, under **Artifacts** download
   `Filemanager-Windows-x64`.
5. Unzip the artifact ZIP, then unzip `Filemanager-win64.zip`.
6. Run `Filemanager.exe`. Windows may show an unsigned-app warning.

If the build is red, open its failed step logs. Do not test file operations
against irreplaceable files. EXE remains an untested development build until
a Windows test verifies navigation, safe copy and Recycle Bin behavior.

## Скриншот реального GPUI-окна

Ручной workflow дополнительно пытается **запустить собранный EXE** в
одноразовом Windows runner и сохранить снимок окна в отдельный
`Filemanager-UI-Screenshot` artifact (`Filemanager-UI.png`).
Это реальный снимок запущенного приложения, **не нарисованный макет**.

На некоторых GitHub-hosted runner графический сеанс недоступен:
тогда снимок не появится и в логе будет предупреждение, но сборка
`Filemanager-Windows-x64` останется доступной. Успешный снимок
не доказывает работоспособность файловых операций — нужен отдельный
безопасный smoke-test из `SMOKE_TEST.md`.

## Local development (optional)

Needs Rust stable, Windows SDK/MSVC C++ Build Tools, and network access for
Cargo dependencies. In PowerShell:

```powershell
rustup show
cargo test -p filemanager-core
cargo run -p filemanager-desktop --release
```

The current assistant sandbox is Linux without Rust and cannot install
dependencies because network DNS is unavailable. Therefore the actual
Windows binary can only be verified via the manual CI run above.

## Current boundaries

- Main stack: **Rust + GPUI**, SQLite history store, Windows Recycle Bin.
- History is metadata-only (no file revisions / restores).
- The prototype limits folder rendering (200 entries per column), and does
  not guarantee catching multiple rapid saves inside the same short interval.
- The recorded account is the observing Windows user, not verified author.
- Copy supports files only; cross-volume folder moves are not implemented.
- Avoid copying data from locations that can change mid-operation until
  the queue becomes crash-resilient.
- No plugins, FTP/SFTP or cloud sync.
