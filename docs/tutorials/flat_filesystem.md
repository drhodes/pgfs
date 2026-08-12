# Tutorial: POSIX Flat Filesystem

**Feature**: `spec.app.FlatFilesystem`

> Users can create, read, write, and delete files through standard
POSIX file operations on the mounted directory.

---

In this tutorial, we will mount a pgfs filesystem and perform basic POSIX file operations.
1. Start the pgfs daemon mounted at `./mnt`.
2. Create a file using `echo 'Hello pgfs' > ./mnt/demo.txt`.
3. Read the file back with `cat ./mnt/demo.txt` and verify the content.
4. Append data using `echo 'More content' >> ./mnt/demo.txt`.
5. Delete the file using `rm ./mnt/demo.txt` and confirm it no longer appears in `ls ./mnt`.
