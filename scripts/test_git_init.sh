#!/usr/bin/env bash
# test_git_init.sh — end-to-end test: git init in a pgfs mount.
# Verifies the original rename-overwrite bug is fixed.
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'
pass() { echo -e "${GREEN}PASS${NC} $*"; }
fail() { echo -e "${RED}FAIL${NC} $*" >&2; exit 1; }

cd "$(dirname "$0")/.."

# 1. Ensure Postgres is running.
if ! pg_ctl -D testdata/pgdata status &>/dev/null; then
    echo "==> Initializing Postgres"
    ./scripts/init_db.sh >/dev/null 2>&1
fi

# 2. Clean slate.
fusermount3 -u -z testdata/mnt 2>/dev/null || true
sleep 0.3
psql -h "$(pwd)/testdata" -d pgfs -c "TRUNCATE entries" >/dev/null 2>&1 || true

# 3. Mount.
cargo build --quiet 2>/dev/null
mkdir -p testdata/mnt
target/debug/pgfs testdata/mnt &
PGFS_PID=$!
for i in $(seq 1 20); do
    if mount | grep -q "testdata/mnt"; then break; fi
    sleep 0.1
done
if ! mount | grep -q "testdata/mnt"; then
    fail "pgfs failed to mount"
fi
echo "pgfs mounted (pid $PGFS_PID)"

cleanup() {
    echo "==> Cleaning up"
    fusermount3 -u -z testdata/mnt 2>/dev/null || true
    wait $PGFS_PID 2>/dev/null || true
}
trap cleanup EXIT

# 4. Run git init — this was the original failing scenario.
echo "==> git init"
cd testdata/mnt
if git init 2>&1; then
    pass "git init succeeded"
else
    fail "git init failed — rename-overwrite bug may still be present"
fi
cd "$(dirname "$0")/.."

# 5. Verify .git structure was created.
echo "==> Verifying .git structure"
for path in .git .git/config .git/HEAD .git/objects .git/refs; do
    if [ ! -e "testdata/mnt/$path" ]; then
        fail "expected $path to exist after git init"
    fi
done
pass ".git structure intact"

echo
echo "=============================="
echo " git init test passed."
echo "=============================="
