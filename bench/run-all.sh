#!/usr/bin/env bash
# Reproduces every table in bench/results. Takes about 50 minutes.
# H=path/to/hangar picks the binary (default: the release build here).
set -u
H=${H:-./target/release/hangar.exe}
[ -x "$H" ] || H=./target/release/hangar

# Five popular servers.
$H bench --sessions 1,5,10 --modes direct,pooled --warm-wave --out bench/results/sim
$H bench --sessions 1,3,5 --modes direct,pooled --browse --no-warmup --out bench/results/sim-browse
$H bench --sessions 5 --modes pooled --no-fast-npx --no-warmup --out bench/results/sim-no-fast-npx
$H bench --sessions 3 --modes pooled --no-fast-npx --browse --no-warmup --out bench/results/sim-browse-no-fast-npx
$H bench --agent claude --sessions 1,3,5 --modes direct,pooled --no-warmup --out bench/results/claude
$H bench --agent claude --sessions 3 --modes direct,pooled --teardown kill --no-warmup --out bench/results/claude-crash

# Playwright and Context7 alone.
P=bench/presets/playwright-context7.json
$H bench --agent claude --preset $P --sessions 1,3,5 --modes direct,pooled --no-warmup --out bench/results/claude-pw-c7
$H bench --preset $P --sessions 1,3,5 --modes direct,pooled --browse --no-warmup --out bench/results/sim-browse-pw-c7
$H bench --preset $P --sessions 1,5,10 --modes direct,pooled --warm-wave --no-warmup --out bench/results/sim-pw-c7

# The same with one shared Playwright browser, which is opt-in
# (share = "native" in config.toml).
$H bench --agent claude --preset $P --sessions 1,3,5 --modes pooled --shared-browser --no-warmup --out bench/results/claude-pw-c7-shared
$H bench --preset $P --sessions 1,3,5 --modes pooled --browse --shared-browser --no-warmup --out bench/results/sim-browse-pw-c7-shared
$H bench --agent claude --sessions 1,3,5 --modes pooled --shared-browser --no-warmup --out bench/results/claude-shared
$H bench --sessions 1,3,5 --modes pooled --browse --shared-browser --no-warmup --out bench/results/sim-browse-shared
