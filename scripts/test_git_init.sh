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

# 3. The rest of the workload (spec/app.py SupportsGitInit): clone a
#    pinned repository (this one, from its local path, so the test needs
#    no network), status, commit with core.fsync, gc, checkout, fsck.
echo "==> git clone, status, commit, gc, checkout, fsck"
PIN=$(git -C "$ROOT" rev-parse HEAD~1)
cd "$MOUNT"
git clone -q --no-hardlinks "$ROOT" clone || fail "git clone"
cd clone
git -c advice.detachedHead=false checkout -q "$PIN" || fail "git checkout of the pinned commit"
git status --porcelain >/dev/null || fail "git status"
git -c core.fsync=all -c user.name=t -c user.email=t@example.com commit -q --allow-empty -m test || fail "git commit"
chmod +x .git/hooks/pre-commit.sample && [ -x .git/hooks/pre-commit.sample ] || fail "chmod +x on a hook"
git gc -q --prune=now || fail "git gc"
git checkout -q -b other "$PIN" || fail "git checkout -b"
git fsck --strict >/dev/null 2>&1 || fail "git fsck"
cd "$ROOT"
pass "clone, status, commit, gc, checkout and fsck"

echo
echo "=============================="
echo " git workload test passed."
echo "=============================="
