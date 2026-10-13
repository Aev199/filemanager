#!/bin/bash
# Drives the window started by run.sh. Coordinates are relative to the
# screenshot origin (1 px left of and 20 px above the client area).
# usage: tools/ui-smoke/ui.sh <display> <outdir> cmd args...
#   click X Y | rclick X Y | dclick X Y | ctrlclick X Y | move X Y
#   key KEYS (xdotool names, e.g. ctrl+k) | type TEXT (ASCII only) | wait SEC | shot NAME
# The first input after start may be lost: click into the window first.
# Frames lag behind input under Xvfb: shot nudges the mouse before capturing.
set -u
D=$1; OUT=$2; shift 2
export DISPLAY=:$D
mkdir -p "$OUT"
read CX CY CW CH < <(xwininfo -name Papka | awk '/Absolute upper-left X/{x=$4}/Absolute upper-left Y/{y=$4}/Width/{w=$2}/Height/{h=$2}END{print x,y,w,h}')
OX=$((CX-1)); OY=$((CY-20)); WW=$((CW+2)); WH=$((CH+30))
while [ $# -gt 0 ]; do
  case $1 in
    click) xdotool mousemove $((OX+$2)) $((OY+$3)) click 1; shift 3;;
    ctrlclick) xdotool mousemove $((OX+$2)) $((OY+$3)) keydown ctrl click 1 keyup ctrl; shift 3;;
    rclick) xdotool mousemove $((OX+$2)) $((OY+$3)) click 3; shift 3;;
    dclick) xdotool mousemove $((OX+$2)) $((OY+$3)) click --repeat 2 --delay 80 1; shift 3;;
    move) xdotool mousemove $((OX+$2)) $((OY+$3)); shift 3;;
    key) xdotool key $2; shift 2;;
    type) xdotool type --delay 30 "$2"; shift 2;;
    wait) sleep $2; shift 2;;
    shot) xdotool mousemove_relative 1 0; sleep 0.5; xdotool mousemove_relative -- -1 0; sleep 0.7
          import -window root -crop ${WW}x${WH}+$OX+$OY "$OUT/$2.png"; shift 2;;
    *) echo "unknown command: $1" >&2; exit 1;;
  esac
done
