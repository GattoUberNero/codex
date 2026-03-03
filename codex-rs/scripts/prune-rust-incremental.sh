#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: prune-rust-incremental.sh [--target-dir PATH] [--keep-per-crate N] [--min-age-hours N] [--apply]

Soft-prune stale Rust incremental caches by keeping only the newest N cache
directories per crate prefix. Defaults are intentionally conservative:

  --target-dir PATH     Cargo target dir (default: ./target)
  --keep-per-crate N    Keep N newest cache dirs per crate prefix (default: 2)
  --min-age-hours N     Only prune dirs older than N hours (default: 12)
  --apply               Delete matches; without this flag, runs in dry-run mode

Examples:
  ./scripts/prune-rust-incremental.sh
  ./scripts/prune-rust-incremental.sh --apply
  ./scripts/prune-rust-incremental.sh --target-dir /tmp/codex-target --keep-per-crate 1 --apply
EOF
}

humanize_bytes() {
  local bytes="$1"
  if command -v numfmt >/dev/null 2>&1; then
    numfmt --to=iec-i --suffix=B "$bytes"
  else
    echo "${bytes}B"
  fi
}

target_dir="target"
keep_per_crate=2
min_age_hours=12
apply_mode=0

while (($# > 0)); do
  case "$1" in
    --target-dir)
      target_dir="${2:?missing value for --target-dir}"
      shift 2
      ;;
    --keep-per-crate)
      keep_per_crate="${2:?missing value for --keep-per-crate}"
      shift 2
      ;;
    --min-age-hours)
      min_age_hours="${2:?missing value for --min-age-hours}"
      shift 2
      ;;
    --apply)
      apply_mode=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if ! [[ "$keep_per_crate" =~ ^[0-9]+$ ]] || ! [[ "$min_age_hours" =~ ^[0-9]+$ ]]; then
  echo "--keep-per-crate and --min-age-hours must be non-negative integers" >&2
  exit 2
fi

incremental_dir="${target_dir%/}/debug/incremental"
if [[ ! -d "$incremental_dir" ]]; then
  echo "No incremental cache found at: $incremental_dir"
  exit 0
fi

now_epoch="$(date +%s)"
min_age_seconds=$((min_age_hours * 3600))

tmp_entries="$(mktemp)"
trap 'rm -f "$tmp_entries"' EXIT

find "$incremental_dir" -mindepth 1 -maxdepth 1 -type d -printf '%T@ %p\n' | sort -nr >"$tmp_entries"

if [[ ! -s "$tmp_entries" ]]; then
  echo "Incremental cache is empty: $incremental_dir"
  exit 0
fi

declare -A seen_per_prefix=()
declare -a prune_paths=()
declare -i total_candidates=0
declare -i total_pruned=0
declare -i total_bytes=0

while read -r mtime path; do
  [[ -n "${path:-}" ]] || continue
  total_candidates+=1
  name="$(basename "$path")"
  prefix="${name%-*}"
  current_seen="${seen_per_prefix[$prefix]:-0}"
  age_seconds=$((now_epoch - ${mtime%.*}))

  if (( current_seen < keep_per_crate )); then
    seen_per_prefix["$prefix"]=$((current_seen + 1))
    continue
  fi

  if (( age_seconds < min_age_seconds )); then
    continue
  fi

  prune_paths+=("$path")
done <"$tmp_entries"

if ((${#prune_paths[@]} == 0)); then
  echo "No stale incremental caches to prune."
  echo "Scanned: $total_candidates directories in $incremental_dir"
  exit 0
fi

echo "Mode        : $([[ "$apply_mode" -eq 1 ]] && echo apply || echo dry-run)"
echo "Target dir  : $target_dir"
echo "Incremental : $incremental_dir"
echo "Policy      : keep newest $keep_per_crate per crate prefix, prune only if older than ${min_age_hours}h"
echo "Candidates  : $total_candidates"
echo "Prune count : ${#prune_paths[@]}"
echo

for path in "${prune_paths[@]}"; do
  bytes_line="$(du -sb "$path" 2>/dev/null | awk '{print $1}')"
  human_line="$(humanize_bytes "${bytes_line:-0}")"
  total_bytes=$((total_bytes + ${bytes_line:-0}))

  if [[ "$apply_mode" -eq 1 ]]; then
    rm -rf -- "$path"
    echo "deleted  ${human_line:-?}  $path"
  else
    echo "would delete  ${human_line:-?}  $path"
  fi
  total_pruned+=1
done

echo
echo "Estimated space: $(humanize_bytes "$total_bytes")"
echo "Finished: $total_pruned directories $([[ "$apply_mode" -eq 1 ]] && echo deleted || echo identified) for pruning."
