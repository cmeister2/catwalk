#!/usr/bin/env bash
set -euo pipefail
tool_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
checkout="$tool_dir/.upstream"
revision=9e8f6d6b5da9371bc791ce3924f364aea81be0ec
if [[ ! -d "$checkout/.git" ]]; then
  mkdir -p "$checkout"
  git -C "$checkout" init --quiet
  git -C "$checkout" remote add origin https://github.com/cadmpeg/cadmpeg.git
  git -C "$checkout" fetch --depth=1 origin "$revision"
  git -C "$checkout" checkout --detach "$revision"
fi
[[ "$(git -C "$checkout" rev-parse HEAD)" == "$revision" ]] || { echo 'Unexpected upstream revision; refusing to patch.' >&2; exit 1; }
patch="$tool_dir/patches/pure-rust-zstd.patch"
if git -C "$checkout" apply --reverse --check "$patch" 2>/dev/null; then
  echo 'Pinned upstream already has the research patch.'
else
  git -C "$checkout" apply --check "$patch"
  git -C "$checkout" apply "$patch"
fi
echo 'Ready. Build or test tools/autodesk_geometry/Cargo.toml with Cargo.'
