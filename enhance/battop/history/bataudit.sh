#!/bin/sh
CLK=$(getconf CLK_TCK 2>/dev/null || echo 100)
UP=$(awk '{print $1}' /proc/uptime)
echo "=== 累计CPU账: CPUs(CPU秒) Life%(生命期CPU占比,持续自旋者高) comm pid — 按 Life% 排 ==="
awk -v CLK="$CLK" -v UP="$UP" '
FNR==1{
  i=match($0, /\)[^)]*$/); rest=substr($0, i+2);
  n=split(rest, a, " "); ut=a[12]; st=a[13]; start=a[20];
  cpu=(ut+st)/CLK; life=UP-start/CLK; pct=(life>0)?100*cpu/life:0;
  pid=$1; cm=$0; sub(/^[0-9]+ \(/,"",cm); sub(/\).*/,"",cm);
  printf "%8.0f  %6.2f  %-20s %s\n", cpu, pct, cm, pid;
}' /proc/[0-9]*/stat | sort -k2 -rn | head -n 20
echo ""
echo "=== 按累计CPU秒排(长期总能耗) ==="
awk -v CLK="$CLK" '
FNR==1{ i=match($0, /\)[^)]*$/); rest=substr($0, i+2);
  n=split(rest, a, " "); ut=a[12]; st=a[13]; cpu=(ut+st)/CLK;
  pid=$1; cm=$0; sub(/^[0-9]+ \(/,"",cm); sub(/\).*/,"",cm);
  printf "%8.0f  %-20s %s\n", cpu, cm, pid; }' /proc/[0-9]*/stat | sort -rn | head -n 15
