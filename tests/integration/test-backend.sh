#!/usr/bin/env bash
set -euo pipefail

BACKEND="${1:?Usage: test-backend.sh <dnf|apt|flatpak>}"
PIKE="${PIKE_BIN:-/pike/pike}"
PASS=0
FAIL=0
SKIP=0

green() { printf '\033[32m%s\033[0m\n' "$*"; }
red()   { printf '\033[31m%s\033[0m\n' "$*"; }
yellow(){ printf '\033[33m%s\033[0m\n' "$*"; }

run_test() {
    local name="$1"
    shift
    printf "  %-40s " "$name"
    if _out=$("$@" 2>&1); then
        green "PASS"
        PASS=$((PASS + 1))
        return 0
    else
        red "FAIL"
        echo "    $_out" | head -5
        FAIL=$((FAIL + 1))
        return 1
    fi
}

skip_test() {
    local name="$1"
    local reason="$2"
    printf "  %-40s " "$name"
    yellow "SKIP ($reason)"
    SKIP=$((SKIP + 1))
}

assert_json() {
    echo "$1" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null
}

assert_json_array_len() {
    local json="$1"
    local min="$2"
    local len
    len=$(echo "$json" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null) || return 1
    [ "$len" -ge "$min" ]
}

assert_not_empty() {
    [ -n "$1" ]
}

assert_contains() {
    echo "$1" | grep -Fqi "$2"
}

echo ""
echo "======================================"
echo "  Pike integration tests: $BACKEND"
echo "======================================"
echo ""

# --- search ---

echo "[search]"

output=$($PIKE search bash --source "$BACKEND" 2>/dev/null) || true
run_test "search bash" assert_not_empty "$output"

output=$($PIKE search bash --source "$BACKEND" --json 2>/dev/null) || true
run_test "search bash --json" assert_json "$output"

run_test "search bash --json has results" assert_json_array_len "$output" 1

output=$($PIKE search xyznonexistent --source "$BACKEND" --json 2>/dev/null) || true
run_test "search nonexistent returns empty" assert_contains "$output" '[]'

# --- list ---

echo ""
echo "[list]"

output=$($PIKE list --json 2>/dev/null) || true
run_test "list --json" assert_json "$output"
run_test "list returns packages" assert_not_empty "$output"
run_test "list has multiple packages" assert_json_array_len "$output" 5

# --- check ---

echo ""
echo "[check]"

output=$($PIKE check --json 2>/dev/null) || true
run_test "check --json" assert_json "$output"

# --- status ---

echo ""
echo "[status]"

output=$($PIKE status --json 2>/dev/null) || true
run_test "status --json" assert_json "$output"

output=$($PIKE status --waybar 2>/dev/null) || true
run_test "status --waybar" assert_json "$output"
run_test "status --waybar has text" assert_contains "$output" '"text"'
run_test "status --waybar has class" assert_contains "$output" '"class"'

# --- repo ---

echo ""
echo "[repo]"

if [ "$BACKEND" = "flatpak" ]; then
    skip_test "repo list --json" "flatpak repos need system bus"
    skip_test "repo list has entries" "flatpak repos need system bus"
else
    output=$($PIKE repo list --source "$BACKEND" --json 2>/dev/null) || true
    run_test "repo list --json" assert_json "$output"
    run_test "repo list has entries" assert_json_array_len "$output" 1
fi

# --- repo add/remove ---

echo ""
echo "[repo add/remove]"

case "$BACKEND" in
    dnf)
        REPO_COUNT_BEFORE=$(echo "$output" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null) || REPO_COUNT_BEFORE=0

        $PIKE repo add "webapp-manager" "kylegospo/webapp-manager" -S dnf -m copr 2>/dev/null || true
        output=$($PIKE repo list --source dnf --json 2>/dev/null) || true
        run_test "repo add (copr)" assert_contains "$output" "copr:copr.fedorainfracloud.org:kylegospo:webapp-manager"

        $PIKE repo disable "copr:copr.fedorainfracloud.org:kylegospo:webapp-manager" -S dnf 2>/dev/null || true
        output=$($PIKE repo list --source dnf --json 2>/dev/null) || true
        run_test "repo disable" assert_contains "$output" '"enabled": false'

        $PIKE repo enable "copr:copr.fedorainfracloud.org:kylegospo:webapp-manager" -S dnf 2>/dev/null || true
        output=$($PIKE repo list --source dnf --json 2>/dev/null) || true
        run_test "repo enable" assert_not_empty "$(echo "$output" | python3 -c "
import sys,json
repos=json.load(sys.stdin)
match=[r for r in repos if 'kylegospo' in r.get('id','') and r.get('enabled',False)]
print('ok' if match else '')
" 2>/dev/null)"

        skip_test "repo remove (dnf)" "dnf does not support repo remove"
        ;;
    apt)
        $PIKE repo add "deadsnakes" "deadsnakes/ppa" -S apt -m ppa 2>/dev/null || true
        output=$($PIKE repo list --source apt --json 2>/dev/null) || true
        run_test "repo add (ppa)" assert_contains "$output" "deadsnakes"

        $PIKE repo remove "ppa:deadsnakes/ppa" -S apt 2>/dev/null || true
        output=$($PIKE repo list --source apt --json 2>/dev/null) || true
        run_test "repo remove (ppa)" test -z "$(echo "$output" | grep -F 'deadsnakes' 2>/dev/null)"
        ;;
    flatpak)
        skip_test "repo add" "flatpak needs system bus"
        skip_test "repo remove" "flatpak needs system bus"
        ;;
esac

# --- install/remove cycle ---

echo ""
echo "[install/remove cycle]"

case "$BACKEND" in
    dnf)
        TEST_PKG="tree"
        MULTI_PKGS="cowsay sl"
        ;;
    apt)
        TEST_PKG="tree"
        MULTI_PKGS="figlet toilet"
        ;;
    flatpak)
        skip_test "install" "flatpak needs system bus"
        skip_test "remove" "flatpak needs system bus"
        skip_test "install multi" "flatpak needs system bus"
        skip_test "remove multi" "flatpak needs system bus"
        skip_test "remove --purge" "flatpak needs system bus"
        TEST_PKG=""
        MULTI_PKGS=""
        ;;
esac

if [ -n "$TEST_PKG" ]; then
    $PIKE remove "$TEST_PKG" --source "$BACKEND" 2>/dev/null || true

    $PIKE install "$TEST_PKG" --source "$BACKEND" 2>/dev/null || true
    run_test "install $TEST_PKG" command -v "$TEST_PKG"

    $PIKE remove "$TEST_PKG" --source "$BACKEND" 2>/dev/null || true
    run_test "remove $TEST_PKG" test ! -x "$(command -v "$TEST_PKG" 2>/dev/null || echo /nonexistent)"
fi

if [ -n "$MULTI_PKGS" ]; then
    # shellcheck disable=SC2086
    $PIKE install $MULTI_PKGS --source "$BACKEND" 2>/dev/null || true
    all_installed=true
    for pkg in $MULTI_PKGS; do
        if ! command -v "$pkg" >/dev/null 2>&1; then
            all_installed=false
        fi
    done
    run_test "install multi ($MULTI_PKGS)" $all_installed

    # shellcheck disable=SC2086
    $PIKE remove $MULTI_PKGS --source "$BACKEND" 2>/dev/null || true
    all_removed=true
    for pkg in $MULTI_PKGS; do
        if command -v "$pkg" >/dev/null 2>&1; then
            all_removed=false
        fi
    done
    run_test "remove multi ($MULTI_PKGS)" $all_removed
fi

# --- remove --purge ---

echo ""
echo "[remove --purge]"

if [ "$BACKEND" = "flatpak" ]; then
    skip_test "remove --purge" "flatpak needs system bus"
elif [ -n "$TEST_PKG" ]; then
    $PIKE install "$TEST_PKG" --source "$BACKEND" 2>/dev/null || true
    $PIKE remove "$TEST_PKG" --source "$BACKEND" --purge 2>/dev/null || true
    run_test "remove --purge $TEST_PKG" test ! -x "$(command -v "$TEST_PKG" 2>/dev/null || echo /nonexistent)"
fi

# --- update ---

echo ""
echo "[update]"

if [ "$BACKEND" = "flatpak" ]; then
    skip_test "update all" "flatpak needs system bus"
    skip_test "update single pkg" "flatpak needs system bus"
    skip_test "update multi pkg" "flatpak needs system bus"
    skip_test "update --source" "flatpak needs system bus"
else
    run_test "update --source $BACKEND" $PIKE update --source "$BACKEND"

    run_test "update single (bash)" $PIKE update bash --source "$BACKEND"

    run_test "update multi (bash coreutils)" $PIKE update bash coreutils --source "$BACKEND"
fi

# --- autoremove ---

echo ""
echo "[autoremove]"

if [ "$BACKEND" = "flatpak" ]; then
    skip_test "autoremove" "flatpak needs system bus"
else
    run_test "autoremove" $PIKE autoremove
fi

# --- results ---

echo ""
echo "======================================"
printf "  Results: \033[32m%s passed\033[0m, " "$PASS"
if [ "$FAIL" -gt 0 ]; then
    printf "\033[31m%s failed\033[0m, " "$FAIL"
else
    printf "0 failed, "
fi
printf "%s skipped\n" "$SKIP"
echo "======================================"
echo ""

exit "$FAIL"
