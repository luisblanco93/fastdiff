#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_dir=$(dirname -- "$script_dir")
cd -- "$repo_dir"

version=$(cargo metadata --no-deps --locked --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "fastdiff-cli"))')
binary_dir=${1:-target/release}
output_dir=${2:-dist}
package="fastdiff-${version}-linux-x64"
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT

mkdir -p -- "$stage/$package/bin" "$output_dir"
for binary in fastdiff fastdiff-gui; do
    if [[ ! -x "$binary_dir/$binary" ]]; then
        printf 'Missing release executable: %s\nBuild with cargo build --release --locked --workspace first.\n' "$binary_dir/$binary" >&2
        exit 1
    fi
    install -m 755 -- "$binary_dir/$binary" "$stage/$package/bin/$binary"
done
mkdir -p -- "$stage/$package/licenses"
install -m 644 -- crates/fastdiff-gui/assets/material/LICENSE "$stage/$package/licenses/material-icons-LICENSE"
tar -czf "$output_dir/$package.tar.gz" -C "$stage" "$package"
printf 'Created %s (%s bytes)\n' "$output_dir/$package.tar.gz" "$(wc -c < "$output_dir/$package.tar.gz")"
