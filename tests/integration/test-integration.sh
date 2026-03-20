#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
RUNTIME="${CONTAINER_RUNTIME:-podman}"

green() { printf '\033[32m%s\033[0m\n' "$*"; }
red()   { printf '\033[31m%s\033[0m\n' "$*"; }
yellow(){ printf '\033[33m%s\033[0m\n' "$*"; }

BACKENDS=("$@")
if [ ${#BACKENDS[@]} -eq 0 ]; then
    BACKENDS=(dnf apt)
fi

echo ""
echo "Pike integration test runner"
echo "Container runtime: $RUNTIME"
echo "Backends: ${BACKENDS[*]}"
echo ""

echo "Building pike (release)..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"
echo ""

TOTAL_PASS=0
TOTAL_FAIL=0

for backend in "${BACKENDS[@]}"; do
    containerfile="$SCRIPT_DIR/Containerfile.$backend"
    if [ ! -f "$containerfile" ]; then
        red "No Containerfile for '$backend' at $containerfile"
        TOTAL_FAIL=$((TOTAL_FAIL + 1))
        continue
    fi

    image="pike-test-$backend"
    echo "--------------------------------------"
    echo "Building $image..."
    echo "--------------------------------------"

    if ! $RUNTIME build -t "$image" -f "$containerfile" "$PROJECT_DIR" 2>&1; then
        red "FAILED to build $image"
        TOTAL_FAIL=$((TOTAL_FAIL + 1))
        continue
    fi

    echo ""
    if $RUNTIME run --rm -t "$image"; then
        green "$backend: ALL PASSED"
        TOTAL_PASS=$((TOTAL_PASS + 1))
    else
        red "$backend: SOME TESTS FAILED"
        TOTAL_FAIL=$((TOTAL_FAIL + 1))
    fi
    echo ""
done

echo "======================================"
echo "  Overall: $TOTAL_PASS backends passed, $TOTAL_FAIL failed"
echo "======================================"

exit "$TOTAL_FAIL"
