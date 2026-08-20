#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/benchmark_hotspots.sh [--quick|--full]

  --quick  Smoke profile: small fixtures, one warmup, three samples
  --full   Decision profile: full fixtures, five warmups, thirty samples (default)
  --help   Show this help
EOF
}

profile=full
case "${1:-}" in
    "" | --full) ;;
    --quick) profile=quick ;;
    --help | -h)
        usage
        exit 0
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
if (( $# > 1 )); then
    usage >&2
    exit 2
fi

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
repo_root=$(cd "$script_dir/.." && pwd -P)
readonly script_dir repo_root profile

if pgrep -x cargo >/dev/null 2>&1 || pgrep -x rustc >/dev/null 2>&1; then
    printf '%s\n' "A Cargo or rustc process is already running; retry when it finishes." >&2
    exit 1
fi

available_kib=$(df -Pk "$repo_root" | awk 'NR == 2 { print $4 }')
if [[ ! "$available_kib" =~ ^[0-9]+$ ]] || (( available_kib < 3 * 1024 * 1024 )); then
    printf '%s\n' "At least 3 GiB of free disk space is required for the release benchmark build." >&2
    exit 1
fi

timestamp=$(date -u '+%Y%m%dT%H%M%SZ')
output_dir="$repo_root/target/benchmarks"
base="$output_dir/hotspots-$timestamp-$profile"
raw_log="$base.log"
jsonl="$base.jsonl"
metadata="$base.meta"
mkdir -p "$output_dir"

commit=$(git -C "$repo_root" rev-parse HEAD)
dirty=false
if [[ -n "$(git -C "$repo_root" status --porcelain)" ]]; then
    dirty=true
fi
{
    printf 'timestamp_utc=%s\n' "$timestamp"
    printf 'profile=%s\n' "$profile"
    printf 'commit=%s\n' "$commit"
    printf 'dirty=%s\n' "$dirty"
    printf 'rustc=%s\n' "$(rustc --version)"
    printf 'host=%s\n' "$(uname -a)"
} >"$metadata"

if [[ "$profile" == quick ]]; then
    export EDITUR_BENCH_QUICK=1
else
    unset EDITUR_BENCH_QUICK || true
fi
export CARGO_INCREMENTAL=0

cd "$repo_root"
{
    cargo test --release --locked --lib 'app::performance::benchmark_' -- \
        --ignored --nocapture --test-threads=1
    cargo test --release --locked --test performance 'benchmark_' -- \
        --ignored --nocapture --test-threads=1
} 2>&1 | tee "$raw_log"

awk '{ marker = index($0, "BENCH {"); if (marker) print substr($0, marker + 6) }' \
    "$raw_log" >"$jsonl"
if [[ ! -s "$jsonl" ]]; then
    printf '%s\n' "Benchmark completed without emitting result records." >&2
    exit 1
fi

rmdir "$repo_root/target/debug/incremental" 2>/dev/null || true
rmdir "$repo_root/target/release/incremental" 2>/dev/null || true

printf 'Results: %s\n' "$jsonl"
printf 'Metadata: %s\n' "$metadata"
printf 'Raw log: %s\n' "$raw_log"
