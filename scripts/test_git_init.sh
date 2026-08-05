#!/usr/bin/env bash
# test_git_init.sh — end-to-end test: git init in a pgfs mount.
# Verifies the original rename-overwrite bug is fixed.
set -euo pipefail
. ./scripts/test_lib.sh

trap default_cleanup EXIT
start_pgfs || fail "pgfs failed to mount"
require_mounted

# 1. Run git init — this was the original failing scenario.
echo "==> git init"
cd "$MOUNT"
if git init 2>&1; then
    pass "git init succeeded"
else
    fail "git init failed — rename-overwrite bug may still be present"
fi
cd "$ROOT"

# 2. Verify .git structure was created.
echo "==> Verifying .git structure"
for path in .git .git/config .git/HEAD .git/objects .git/refs; do
    if [ ! -e "$MOUNT/$path" ]; then
        fail "expected $path to exist after git init"
    fi
done
pass ".git structure intact"

echo
echo "=============================="
echo " git init test passed."
echo "=============================="
