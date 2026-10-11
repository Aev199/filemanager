#!/bin/bash
# Starts the debug build in a virtual X display for screenshots (Linux only).
# usage: tools/ui-smoke/run.sh <display-number> [WxH]
# Needs Xvfb, openbox, xcompmgr. Profile and settings go to /tmp/fm-smoke-*.
set -u
D=$1; G=${2:-1440x900}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
pkill -x filemanager-des 2>/dev/null; pkill -f "^Xvfb :$D( |$)" 2>/dev/null; rm -f /tmp/.X$D-lock; sleep 1
(Xvfb :$D -screen 0 ${G}x24 >/dev/null 2>&1 &)
sleep 1
(DISPLAY=:$D openbox >/dev/null 2>&1 &)
(DISPLAY=:$D xcompmgr >/dev/null 2>&1 &)
mkdir -p /tmp/fm-smoke-home /tmp/fm-smoke-appdata
# Sample tree (names only matter): folders, document kinds, file versions.
H=/tmp/fm-smoke-home
mkdir -p "$H/Документы/Проекты/Атлас" "$H/Документы/Отчёты" "$H/Загрузки" "$H/Изображения"
for f in "Документы/Пояснительная записка.docx" "Документы/Пояснительная записка_v_1_5.docx" \
         "Документы/Пояснительная записка_v_1_6.docx" "Документы/Расчётная модель.xlsx" \
         "Документы/План работ.pdf" "Документы/notes.txt" "Загрузки/archive.zip" \
         "Изображения/Горы.jpg" "Документы/Проекты/Атлас/чертёж.dwg"; do
  [ -e "$H/$f" ] || echo "sample" > "$H/$f"
done
(cd /tmp/fm-smoke-home && DISPLAY=:$D HOME=/tmp/fm-smoke-home LOCALAPPDATA=/tmp/fm-smoke-appdata \
  timeout 900 "$ROOT/target/debug/filemanager-desktop" >/tmp/fm-smoke-app.log 2>&1 &)
