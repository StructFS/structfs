#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
for release_python in python3.12 python3.13 python3.14 python3.11 python3; do
    if command -v "$release_python" >/dev/null 2>&1 && "$release_python" -c 'import tomllib, tarfile; assert hasattr(tarfile, "data_filter")' >/dev/null 2>&1; then
        exec "$release_python" "$SCRIPT_DIR/check-featherweight-release.py"
    fi
done
echo 'The package gate requires Python with tomllib and tarfile.data_filter (3.12 recommended).' >&2
exit 1
