#!/usr/bin/env bash
# run_tests.sh — run the full pgfs integration test suite.
#
# Every scripts/test_*.sh is a self-contained integration test: it
# sources scripts/test_lib.sh, mounts pgfs at testdata/mnt, runs its
# checks, and unmounts. This runner executes each in isolation (so one
# failure can't abort the rest), reports a pass/fail line per test, and
# prints a summary — the same shape as `./check -g quick` in xfstests.
set -uo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

# test_lib.sh is a harness, not a test — exclude it from the run.
TESTS=()
for t in scripts/test_*.sh; do
    case "$(basename "$t")" in
        test_lib.sh) continue ;;
        *) TESTS+=("$t") ;;
    esac
done
TOTAL=${#TESTS[@]}
PASSED=0
FAILED=0
FAILED_NAMES=()

for t in "${TESTS[@]}"; do
    name=$(basename "$t")
    log="$ROOT/testdata/$name.log"
    echo "── $name"
    if timeout 300 bash "$t" >"$log" 2>&1; then
        echo -e "   ${GREEN}PASS${NC}"
        PASSED=$((PASSED + 1))
    else
        echo -e "   ${RED}FAIL${NC} (log: testdata/$name.log)"
        FAILED=$((FAILED + 1))
        FAILED_NAMES+=("$name")
        tail -8 "$log" | sed 's/^/     /'
    fi
    # A timed-out test is SIGTERM'd without running its EXIT trap, so its
    # backgrounded daemon would linger. Sweep any pgfs daemon and mount so
    # the next test starts clean; wait for the mount to actually vanish
    # (fusermount3 -u -z is async) before proceeding.
    pkill -f "$ROOT/target/debug/pgfs" 2>/dev/null || true
    fusermount3 -u -z "$ROOT/testdata/mnt" 2>/dev/null || true
    for _ in $(seq 1 40); do
        grep -q " $ROOT/testdata/mnt fuse" /proc/mounts 2>/dev/null || break
        sleep 0.1
    done
done

if [ "$TOTAL" -eq 0 ]; then
    echo "no integration tests found (scripts/test_*.sh)" >&2
    exit 1
fi

echo
echo "══════════════════════════════════"
if [ "$FAILED" -eq 0 ]; then
    echo -e "${GREEN}All $TOTAL integration tests passed.${NC}"
else
    echo -e "${RED}$FAILED of $TOTAL integration tests FAILED:${NC}"
    printf '  %s\n' "${FAILED_NAMES[@]}"
fi
echo "══════════════════════════════════"
[ "$FAILED" -eq 0 ]
