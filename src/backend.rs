//! Core storage backend trait and data types for pgfs.
//!
//! Decouples the FUSE filesystem layer (fs.rs) from the underlying
//! database access mechanics (LocalBackend via pgrx or RemoteBackend via tokio-postgres).

use async_trait::async_trait;
use std::time::SystemTime;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Entry not found")]
    NotFound,
    #[error("Entry already exists")]
    AlreadyExists,
    #[error("Directory not empty")]
    NotEmpty,
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
    #[error("Database error: {0}")]
    DatabaseError(String),
    #[error("IO error: {0}")]
    IoError(std::io::ErrorKind),
}

impl From<StorageError> for libc::c_int {
    fn from(err: StorageError) -> Self {
        match err {
            StorageError::NotFound => libc::ENOENT,
            StorageError::AlreadyExists => libc::EEXIST,
            StorageError::NotEmpty => libc::ENOTEMPTY,
            StorageError::InvalidArgument(_) => libc::EINVAL,
            StorageError::DatabaseError(_) => libc::EIO,
            StorageError::IoError(k) => match k {
                std::io::ErrorKind::NotFound => libc::ENOENT,
                std::io::ErrorKind::AlreadyExists => libc::EEXIST,
                std::io::ErrorKind::PermissionDenied => libc::EACCES,
                std::io::ErrorKind::InvalidInput => libc::EINVAL,
                _ => libc::EIO,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File = 1,
    Directory = 2,
    Symlink = 3,
}

impl From<FileKind> for fuser::FileType {
    fn from(kind: FileKind) -> Self {
        match kind {
            FileKind::File => fuser::FileType::RegularFile,
            FileKind::Directory => fuser::FileType::Directory,
            FileKind::Symlink => fuser::FileType::Symlink,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileAttr {
    pub ino: u64,
    pub size: u64,
    pub blocks: u64,
    pub atime: SystemTime,
    pub mtime: SystemTime,
    pub ctime: SystemTime,
    pub kind: FileKind,
    pub perm: u16,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u32,
    pub flags: u32,
}

impl From<FileAttr> for fuser::FileAttr {
    fn from(attr: FileAttr) -> Self {
        fuser::FileAttr {
            ino: attr.ino,
            size: attr.size,
            blocks: attr.blocks,
            atime: attr.atime,
            mtime: attr.mtime,
            ctime: attr.ctime,
            crtime: attr.ctime,
            kind: attr.kind.into(),
            perm: attr.perm,
            nlink: attr.nlink,
            uid: attr.uid,
            gid: attr.gid,
            rdev: attr.rdev,
            flags: attr.flags,
            blksize: 131072,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub ino: u64,
    pub name: String,
    pub kind: FileKind,
}

#[allow(dead_code)]
#[derive(Debug, Default, Clone)]
pub struct SetAttrInput {
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub size: Option<u64>,
    pub atime: Option<SystemTime>,
    pub mtime: Option<SystemTime>,
}

/// Helper function to zero-pad unaligned or sparse block writes (SparseWritePaddingReq).
#[allow(dead_code)]
pub fn prepare_sparse_block_write(
    existing_data: Option<Vec<u8>>,
    offset_in_block: usize,
    payload: &[u8],
) -> Vec<u8> {
    let mut block = existing_data.unwrap_or_default();
    let required_len = offset_in_block + payload.len();

    if block.len() < required_len {
        block.resize(required_len, 0);
    }

    block[offset_in_block..required_len].copy_from_slice(payload);
    block
}

#[allow(dead_code)]
#[async_trait]
pub trait PgfsBackend: Send + Sync {
    /// Metadata Operations
    async fn lookup(&self, parent_ino: u64, name: &str) -> Result<Option<FileAttr>, StorageError>;
    async fn getattr(&self, ino: u64) -> Result<FileAttr, StorageError>;
    async fn setattr(&self, ino: u64, attr: SetAttrInput) -> Result<FileAttr, StorageError>;

    /// Directory Operations
    async fn readdir(&self, ino: u64, offset: i64) -> Result<Vec<DirEntry>, StorageError>;
    async fn mkdir(
        &self,
        parent_ino: u64,
        name: &str,
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> Result<FileAttr, StorageError>;
    async fn rmdir(&self, parent_ino: u64, name: &str) -> Result<(), StorageError>;

    /// File Mutation Operations
    async fn create(
        &self,
        parent_ino: u64,
        name: &str,
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> Result<FileAttr, StorageError>;
    async fn unlink(&self, parent_ino: u64, name: &str) -> Result<(), StorageError>;
    async fn rename(
        &self,
        old_parent: u64,
        old_name: &str,
        new_parent: u64,
        new_name: &str,
    ) -> Result<(), StorageError>;

    /// Chunked Block I/O Operations
    async fn read_block(&self, ino: u64, block_no: u32) -> Result<Vec<u8>, StorageError>;
    async fn write_block(
        &self,
        ino: u64,
        block_no: u32,
        data: &[u8],
        offset_in_block: u32,
    ) -> Result<u32, StorageError>;
    async fn truncate_blocks(&self, ino: u64, new_size: u64) -> Result<(), StorageError>;

    /// Database-Native Offloading Operations
    async fn copy_file_range(
        &self,
        ino_in: u64,
        off_in: u64,
        ino_out: u64,
        off_out: u64,
        len: u64,
    ) -> Result<u64, StorageError> {
        let _ = (ino_in, off_in, ino_out, off_out, len);
        Err(StorageError::InvalidArgument(
            "copy_file_range offloading not implemented for this backend".into(),
        ))
    }

    async fn fallocate(
        &self,
        ino: u64,
        offset: u64,
        length: u64,
        mode: u32,
    ) -> Result<(), StorageError> {
        let _ = (ino, offset, length, mode);
        Err(StorageError::InvalidArgument(
            "fallocate offloading not implemented for this backend".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_error_to_libc_errno() {
        assert_eq!(libc::c_int::from(StorageError::NotFound), libc::ENOENT);
        assert_eq!(libc::c_int::from(StorageError::AlreadyExists), libc::EEXIST);
        assert_eq!(libc::c_int::from(StorageError::NotEmpty), libc::ENOTEMPTY);
        assert_eq!(
            libc::c_int::from(StorageError::InvalidArgument("test".into())),
            libc::EINVAL
        );
        assert_eq!(
            libc::c_int::from(StorageError::DatabaseError("db error".into())),
            libc::EIO
        );
    }

    #[test]
    fn test_file_kind_conversion() {
        assert_eq!(
            fuser::FileType::from(FileKind::File),
            fuser::FileType::RegularFile
        );
        assert_eq!(
            fuser::FileType::from(FileKind::Directory),
            fuser::FileType::Directory
        );
        assert_eq!(
            fuser::FileType::from(FileKind::Symlink),
            fuser::FileType::Symlink
        );
    }

    #[test]
    fn test_prepare_sparse_block_write_zero_padding() {
        // Write 4 bytes at offset 10 into a brand new block
        let payload = [1u8, 2, 3, 4];
        let block = prepare_sparse_block_write(None, 10, &payload);

        assert_eq!(block.len(), 14);
        assert_eq!(&block[0..10], &[0u8; 10]); // Preceding bytes are zero-padded
        assert_eq!(&block[10..14], &[1u8, 2, 3, 4]); // Payload copied at offset
    }

    #[test]
    fn test_prepare_sparse_block_write_overwrite_existing() {
        let existing = vec![9u8; 20];
        let payload = [5u8, 6, 7];
        let block = prepare_sparse_block_write(Some(existing), 5, &payload);

        assert_eq!(block.len(), 20);
        assert_eq!(&block[0..5], &[9u8; 5]);
        assert_eq!(&block[5..8], &[5u8, 6, 7]);
        assert_eq!(&block[8..20], &[9u8; 12]);
    }
}
