
use serde::{Deserialize, Serialize};

// Re-export Qid so implementations can use it
pub use super::Qid;

/// Metadata for a file/directory
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileAttr {
    pub qid: Qid,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u64,
    pub rdev: u64,
    pub size: u64,
    pub blksize: u32,
    pub blocks: u64,
    pub atime: (u64, u64), // seconds, nanoseconds
    pub mtime: (u64, u64),
    pub ctime: (u64, u64),
}

/// A directory entry
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DirEntry {
    pub qid: Qid,
    pub offset: u64, // Offset to next entry
    pub type_: u8,
    pub name: String,
}

/// The FileSystem trait abstracts the underlying storage
pub trait FileSystem: Send + Sync {
    /// Initialize and return the root QID
    fn attach(&mut self) -> Result<Qid, u32>;

    /// Walk somewhat mirrors 9P walk, but step by step
    /// Returns the QID of the child if found
    fn walk(&mut self, parent_qid: &Qid, name: &str) -> Result<Qid, u32>;

    /// Get attributes for a file
    fn getattr(&mut self, qid: &Qid) -> Result<FileAttr, u32>;

    /// Open a file (check permissions, prepare handles)
    fn open(&mut self, qid: &Qid, flags: u32) -> Result<(), u32>;

    /// Create a file
    fn create(&mut self, parent_qid: &Qid, name: &str, mode: u32, flags: u32) -> Result<Qid, u32>;

    /// Create a directory
    fn mkdir(&mut self, parent_qid: &Qid, name: &str, mode: u32) -> Result<Qid, u32>;

    /// Read from a file
    fn read(&mut self, qid: &Qid, offset: u64, count: u32) -> Result<Vec<u8>, u32>;

    /// Write to a file
    fn write(&mut self, qid: &Qid, offset: u64, data: &[u8]) -> Result<u32, u32>;

    /// Read directory entries
    fn readdir(&mut self, qid: &Qid, offset: u64, count: u32) -> Result<Vec<DirEntry>, u32>;

    /// Remove a file
    fn remove(&mut self, qid: &Qid) -> Result<(), u32>;
    
    /// Rename/Move a file
    fn rename(&mut self, qid: &Qid, new_dir: &Qid, new_name: &str) -> Result<(), u32>;
}
