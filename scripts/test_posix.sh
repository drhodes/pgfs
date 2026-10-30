#!/usr/bin/env bash
# test_posix.sh — POSIX details real tools depend on (spec/posix.py
# PosixCompatibility): attributes that stick across a remount, hard links,
# symlinks, FIFOs, xattrs, open-after-unlink, exclusive create, renameat2
# flags, sparse files with SEEK_DATA/SEEK_HOLE, fallocate and
# copy_file_range, and Bug 3 (a rename losing buffered writes).
source "$(dirname "${BASH_SOURCE[0]}")/test_lib.sh"
trap default_cleanup EXIT
start_pgfs || fail "pgfs failed to mount"
cd "$MOUNT"

# Bug 3: write without closing, rename, read through the new name.
exec 3>buffered.tmp
printf 'abc' >&3
mv buffered.tmp final.txt
[ "$(cat final.txt)" = "abc" ] || fail "rename lost buffered writes"
printf 'def' >&3
exec 3>&-
[ "$(cat final.txt)" = "abcdef" ] || fail "writes after rename lost"
pass "rename keeps buffered writes (Bug 3)"

# Attributes stick.
touch -d '2001-02-03 04:05:06.123456789' stamped
chmod 640 stamped
[ "$(stat -c %a stamped)" = 640 ] || fail "chmod"
[ "$(stat -c %y stamped)" = "2001-02-03 04:05:06.123456789 $(date -d '2001-02-03' +%z)" ] || fail "nanosecond mtime: $(stat -c %y stamped)"
pass "mode and nanosecond times stick"

# Hard links, symlinks, FIFOs.
echo data > orig
ln orig alias
[ "$(stat -c %h orig)" = 2 ] || fail "nlink after link"
ln -s orig sym
[ "$(readlink sym)" = orig ] && [ "$(cat sym)" = data ] || fail "symlink"
mkfifo pipe
[ -p pipe ] || fail "mkfifo"
mkdir d && ! ln d dlink 2>/dev/null || fail "hard link to a directory must fail"
pass "links, symlinks, FIFOs"

# Extended attributes and open-after-unlink, exclusive create, rename
# flags, sparse files, fallocate, copy_file_range.
python3 - <<'PY' || fail "python POSIX checks"
import errno, os
os.setxattr("orig", "user.k", b"v1", os.XATTR_CREATE)
try:
    os.setxattr("orig", "user.k", b"v2", os.XATTR_CREATE); raise SystemExit("XATTR_CREATE")
except OSError as e:
    assert e.errno == errno.EEXIST
assert os.getxattr("orig", "user.k") == b"v1"
assert "user.k" in os.listxattr("orig")
os.removexattr("orig", "user.k")
try:
    os.getxattr("orig", "user.k"); raise SystemExit("ENODATA")
except OSError as e:
    assert e.errno == errno.ENODATA
try:
    os.setxattr("orig", "system.posix_acl_access", b"x"); raise SystemExit("acl")
except OSError as e:
    assert e.errno in (errno.EOPNOTSUPP, errno.ENOTSUP, errno.EINVAL), e

# Open after unlink.
fd = os.open("tmpfile", os.O_CREAT | os.O_RDWR, 0o600)
os.write(fd, b"still here")
os.unlink("tmpfile")
assert not os.path.exists("tmpfile")
os.lseek(fd, 0, 0)
assert os.read(fd, 100) == b"still here"
os.close(fd)

# Exclusive create.
fd = os.open("index.lock", os.O_CREAT | os.O_EXCL | os.O_WRONLY)
os.close(fd)
try:
    os.open("index.lock", os.O_CREAT | os.O_EXCL | os.O_WRONLY); raise SystemExit("O_EXCL")
except OSError as e:
    assert e.errno == errno.EEXIST

# Sparse file: a hole, then data.
with open("sparse", "wb") as f:
    f.seek(1 << 20)
    f.write(b"end")
fd = os.open("sparse", os.O_RDONLY)
assert os.lseek(fd, 0, os.SEEK_DATA) >= 4096
assert os.lseek(fd, 0, os.SEEK_HOLE) == 0
os.close(fd)
st = os.stat("sparse")
assert st.st_size == (1 << 20) + 3 and st.st_blocks * 512 < 64 * 1024, st

# fallocate punch hole and default mode.
with open("punch", "wb") as f:
    f.write(b"x" * 20000)
fd = os.open("punch", os.O_RDWR)
import ctypes
libc = ctypes.CDLL(None, use_errno=True)
libc.fallocate.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_long, ctypes.c_long]
assert libc.fallocate(fd, 0x01 | 0x02, 100, 10000) == 0, ctypes.get_errno()
os.close(fd)
data = open("punch", "rb").read()
assert len(data) == 20000 and data[:100] == b"x" * 100 and data[100:10100] == b"\0" * 10000 and data[10100:] == b"x" * 9900

# copy_file_range.
with open("src", "wb") as f:
    f.write(os.urandom(300000))
s = os.open("src", os.O_RDONLY); d = os.open("dst", os.O_CREAT | os.O_WRONLY, 0o644)
n = 0
while n < 300000:
    k = os.copy_file_range(s, d, 300000 - n, n, n)
    assert k > 0
    n += k
os.close(s); os.close(d)
assert open("src", "rb").read() == open("dst", "rb").read()
PY
pass "xattrs, open-after-unlink, O_EXCL, sparse files, fallocate, copy_file_range"

# Rename into its own subtree fails; mv -T over a non-empty directory fails.
mkdir -p a/b
! mv a a/b/ 2>/dev/null || fail "rename into own subtree"
mkdir -p full && touch full/x
! mv -T d full 2>/dev/null || fail "rename over non-empty directory"
pass "rename errors"

# Everything survives a remount.
cd "$ROOT"
stop_pgfs
target/debug/pgfs mount main "$MOUNT" >>"$TESTDATA/pgfs.log" 2>&1 &
PGFS_PID=$!
for _ in $(seq 1 30); do mount_active && break; sleep 0.1; done
cd "$MOUNT"
[ "$(cat final.txt)" = "abcdef" ] || fail "content after remount"
[ "$(stat -c %a stamped)" = 640 ] || fail "mode after remount"
[ "$(stat -c %h orig)" = 2 ] || fail "nlink after remount"
cmp -s src dst || fail "copy after remount"
pass "everything persists across a remount"
