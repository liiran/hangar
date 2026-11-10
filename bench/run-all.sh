#!/usr/bin/env bash
# Reproduces every table in bench/results. Takes about 15 minutes.
set -u
H=./target/release/hangar.exe
[ -x "$H" ] || H=./target/release/hangar
$H bench --sessions 1,5,10 --modes direct,pooled --warm-wave --out bench/results/sim
$H bench --sessions 1,3,5 --modes direct,pooled --browse --no-warmup --out bench/results/sim-browse
$H bench --sessions 5 --modes pooled --no-fast-npx --no-warmup --out bench/results/sim-no-fast-npx
$H bench --sessions 3 --modes pooled --no-fast-npx --browse --no-warmup --out bench/results/sim-browse-no-fast-npx
$H bench --agent claude --sessions 1,3,5 --modes direct,pooled --no-warmup --out bench/results/claude
$H bench --agent claude --sessions 3 --modes direct,pooled --teardown kill --no-warmup --out bench/results/claude-crash
