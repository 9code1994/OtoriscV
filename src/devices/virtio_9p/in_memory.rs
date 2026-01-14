
use std::collections::{HashMap, HashSet};
use super::{Qid, P9_QTDIR, P9_QTFILE, P9_QTSYMLINK};
use super::filesystem::{FileSystem, FileAttr, DirEntry};
use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Inode {
    pub qid: Qid,
    pub name: String,
    pub size: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime: u64,
    pub atime: u64,
    pub ctime: u64,
    pub content: InodeContent,
    pub children: Vec<u64>,
    pub parent: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum InodeContent {
    Inline(Vec<u8>),
    Hash(String),
    Directory,
    Symlink(String),
}

impl Inode {
    pub fn is_dir(&self) -> bool {
        (self.mode & 0o170000) == 0o040000
    }
}

/// In-Memory Filesystem implementation
#[derive(Serialize, Deserialize)]
pub struct InMemoryFileSystem {
    pub inodes: HashMap<u64, Inode>,
    pub next_inode: u64,
    pub blob_cache: HashMap<String, Vec<u8>>,
    pub missing_blobs: HashSet<String>,
}

impl InMemoryFileSystem {
    pub fn new() -> Self {
        let mut fs = InMemoryFileSystem {
            inodes: HashMap::new(),
            next_inode: 1,
            blob_cache: HashMap::new(),
            missing_blobs: HashSet::new(),
        };
        
        // Create root
        let root_qid = Qid::new(P9_QTDIR, 0);
        let root = Inode {
            qid: root_qid,
            name: String::new(),
            size: 0,
            mode: 0o40755,
            uid: 0,
            gid: 0,
            mtime: 0,
            atime: 0,
            ctime: 0,
            content: InodeContent::Directory,
            children: Vec::new(),
            parent: 0,
        };
        fs.inodes.insert(0, root);
        fs
    }

    fn alloc_inode(&mut self) -> u64 {
        let id = self.next_inode;
        self.next_inode += 1;
        id
    }
}

impl FileSystem for InMemoryFileSystem {
    fn attach(&mut self) -> Result<Qid, u32> {
        let root = self.inodes.get(&0).unwrap();
        Ok(root.qid)
    }

    fn walk(&mut self, parent_qid: &Qid, name: &str) -> Result<Qid, u32> {
        let parent_path = parent_qid.path;
        let parent = self.inodes.get(&parent_path).ok_or(2u32)?; // ENOENT
        
        if !parent.is_dir() {
            return Err(20); // ENOTDIR
        }

        if name == "." {
            return Ok(parent.qid);
        } else if name == ".." {
            let grand_parent_path = parent.parent;
            let grand_parent = self.inodes.get(&grand_parent_path).ok_or(2u32)?;
            return Ok(grand_parent.qid);
        }

        for &child_path in &parent.children {
            if let Some(child) = self.inodes.get(&child_path) {
                if child.name == name {
                    return Ok(child.qid);
                }
            }
        }

        Err(2) // ENOENT
    }

    fn getattr(&mut self, qid: &Qid) -> Result<FileAttr, u32> {
        let inode = self.inodes.get(&qid.path).ok_or(2u32)?;
        
        Ok(FileAttr {
            qid: inode.qid,
            mode: inode.mode,
            uid: inode.uid,
            gid: inode.gid,
            nlink: 1,
            rdev: 0,
            size: inode.size,
            blksize: 4096,
            blocks: (inode.size + 511) / 512,
            atime: (inode.atime, 0),
            mtime: (inode.mtime, 0),
            ctime: (inode.ctime, 0),
        })
    }
    
    fn open(&mut self, _qid: &Qid, _flags: u32) -> Result<(), u32> {
        // In-memory doesn't need to do much for open
        Ok(())
    }

    fn create(&mut self, parent_qid: &Qid, name: &str, mode: u32, _flags: u32) -> Result<Qid, u32> {
        let parent_path = parent_qid.path;
        
        // Verify parent exists and is dir
        if !self.inodes.contains_key(&parent_path) {
            return Err(2);
        }
        
        // Check if name exists
        let parent = self.inodes.get(&parent_path).unwrap();
        for &child_path in &parent.children {
            if let Some(child) = self.inodes.get(&child_path) {
                if child.name == name {
                    return Err(17); // EEXIST
                }
            }
        }
        
        // Allocate new inode
        let new_path = self.alloc_inode();
        let is_dir = (mode & 0o170000) == 0o040000;
        let qtype = if is_dir { P9_QTDIR } else { P9_QTFILE };
        let qid = Qid::new(qtype, new_path);
        
        let content = if is_dir {
            InodeContent::Directory
        } else {
            InodeContent::Inline(Vec::new())
        };
        
        let inode = Inode {
            qid,
            name: name.to_string(),
            size: 0,
            mode,
            uid: 0,
            gid: 0,
            mtime: 0,
            atime: 0,
            ctime: 0,
            content,
            children: Vec::new(),
            parent: parent_path,
        };
        
        self.inodes.insert(new_path, inode);
        
        // Add to parent
        let parent = self.inodes.get_mut(&parent_path).unwrap();
        parent.children.push(new_path);
        
        Ok(qid)
    }

    fn mkdir(&mut self, parent_qid: &Qid, name: &str, mode: u32) -> Result<Qid, u32> {
        // Helper that reuses create logic but ensures DIR bit
        self.create(parent_qid, name, mode | 0o040000, 0)
    }

    fn read(&mut self, qid: &Qid, offset: u64, count: u32) -> Result<Vec<u8>, u32> {
        let inode = self.inodes.get(&qid.path).ok_or(2u32)?;
        
        match &inode.content {
            InodeContent::Inline(data) => {
                let start = offset as usize;
                if start >= data.len() {
                    return Ok(Vec::new());
                }
                let end = std::cmp::min(start + count as usize, data.len());
                Ok(data[start..end].to_vec())
            },
            InodeContent::Hash(hash) => {
                if let Some(data) = self.blob_cache.get(hash) {
                    let start = offset as usize;
                    if start >= data.len() {
                        return Ok(Vec::new());
                    }
                    let end = std::cmp::min(start + count as usize, data.len());
                    Ok(data[start..end].to_vec())
                } else {
                    // Missing blob
                    // self.missing_blobs.insert(hash.clone()); // Mutability issue?
                    // In a real implementation we would signal missing blob.
                    // For now return IO error or handle it.
                    // Implementing "lazy load" via trait is tricky without async or callback.
                    Err(5) // EIO (or custom for "try again")
                }
            },
            _ => Err(5), // EIO
        }
    }

    fn write(&mut self, qid: &Qid, offset: u64, data: &[u8]) -> Result<u32, u32> {
        let inode = self.inodes.get_mut(&qid.path).ok_or(2u32)?;
        
        if let InodeContent::Inline(ref mut content) = inode.content {
            let end = offset as usize + data.len();
            if end > content.len() {
                content.resize(end, 0);
            }
            content[offset as usize..end].copy_from_slice(data);
            inode.size = content.len() as u64;
            inode.mtime = 0; // Update time (todo)
            Ok(data.len() as u32)
        } else {
            Err(5) // EIO
        }
    }

    fn readdir(&mut self, qid: &Qid, offset: u64, count: u32) -> Result<Vec<DirEntry>, u32> {
        let inode = self.inodes.get(&qid.path).ok_or(2u32)?;
        
        if !inode.is_dir() {
            return Err(20);
        }
        
        let mut entries = Vec::new();
        let mut current_pos = 0;
        
        for &child_path in &inode.children {
            if current_pos >= offset {
                 if let Some(child) = self.inodes.get(&child_path) {
                     entries.push(DirEntry {
                         qid: child.qid,
                         offset: current_pos + 1,
                         type_: child.qid.qtype,
                         name: child.name.clone(),
                     });
                     
                     // Approximate size check (not exact 9P wire size, but close enough for logic)
                     // In the outer loop we serialize and check real size.
                     // Here we just return all relevant entries and let caller paginate?
                     // 9P `count` is byte limit. It's hard to guess exact bytes here.
                     // The trait returns Vec<DirEntry> which the caller serializes until full.
                 }
            }
            current_pos += 1;
        }
        
        Ok(entries)
    }

    fn remove(&mut self, qid: &Qid) -> Result<(), u32> {
        let inode = self.inodes.get(&qid.path).ok_or(2u32)?;
        let parent_path = inode.parent;
        
        if !inode.children.is_empty() {
            return Err(39); // ENOTEMPTY
        }
        
        // Remove from parent
        if let Some(parent) = self.inodes.get_mut(&parent_path) {
            parent.children.retain(|&x| x != qid.path);
        }
        
        self.inodes.remove(&qid.path);
        Ok(())
    }

    fn rename(&mut self, qid: &Qid, new_dir: &Qid, new_name: &str) -> Result<(), u32> {
        // Simplified rename
        let inode = self.inodes.get_mut(&qid.path).ok_or(2u32)?;
        inode.name = new_name.to_string();
        
        let old_parent = inode.parent;
        inode.parent = new_dir.path;
        
        if old_parent != new_dir.path {
            // Move references
             if let Some(p) = self.inodes.get_mut(&old_parent) {
                p.children.retain(|&x| x != qid.path);
            }
             if let Some(p) = self.inodes.get_mut(&new_dir.path) {
                p.children.push(qid.path);
            }
        }
        
        Ok(())
    }
}
