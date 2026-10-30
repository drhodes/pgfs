#!/usr/bin/env bash
# test_suid.sh — set-id bits and device nodes are opt-in
# (spec/security.py SetIdAndDevicesOptIn). Unprivileged: --suid and --dev
# are refused before mounting, and a mount is nosuid,nodev. As root: the
# flags take effect, and a root mount without them is still nosuid,nodev.
source "$(dirname "${BASH_SOURCE[0]}")/test_lib.sh"
trap default_cleanup EXIT
mount_flags() { grep " $MOUNT fuse" /proc/mounts | awk '{print $4}'; }

if [ "$(id -u)" != 0 ]; then
    cargo build --quiet 2>/dev/null
    mkdir -p "$MOUNT"
    out=$(target/debug/pgfs mount main "$MOUNT" --suid 2>&1) && code=0 || code=$?
    [ "$code" = 2 ] || fail "--suid without root exited $code, want 2"
    echo "$out" | grep -q "root" || fail "--suid without root: unclear error: $out"
    ! mount_active || fail "--suid without root mounted something"
    if target/debug/pgfs mount main "$MOUNT" --dev >/dev/null 2>&1; then fail "--dev without root mounted"; fi
    pass "--suid and --dev are refused without root, before mounting"
fi

start_pgfs || fail "mount"
flags=$(mount_flags)
case ",$flags," in *,nosuid,*) ;; *) fail "default mount is not nosuid: $flags" ;; esac
case ",$flags," in *,nodev,*) ;; *) fail "default mount is not nodev: $flags" ;; esac
pass "a mount is nosuid,nodev by default ($flags)"

if [ "$(id -u)" = 0 ]; then
    stop_pgfs
    PGFS_MOUNT_ARGS="--allow-other --suid --dev" start_pgfs || fail "mount --suid --dev"
    flags=$(mount_flags)
    case ",$flags," in *,nosuid,*|*,nodev,*) fail "--suid --dev not applied: $flags" ;; esac
    grep -q "WARN.*--suid" "$TESTDATA/pgfs.log" || fail "no WARN for --suid"
    pass "--suid and --dev take effect for root ($flags)"
else
    echo "SKIP: the root half of this test needs root"
fi
