
use std::fs;
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::sync::{RwLock, Arc};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::time::SystemTime;

use super::{Qid, P9_QTDIR, P9_QTFILE, P9_QTSYMLINK};
use super::filesystem::{FileSystem, FileAttr, DirEntry};

pub struct HostFileSystem {
    root_path: PathBuf,
    // Mapping from QID path (u64) to host PathBuf
    paths: Arc<RwLock<HashMap<u64, PathBuf>>>,
    // Mapping from host PathBuf (canonical) to QID path to ensure stability
    // Actually hashing the path might be easier? Or just counting.
    // For now simple counter.
    ids: Arc<RwLock<HashMap<PathBuf, u64>>>,
    next_id: Arc<RwLock<u64>>,
}

use serde::{Serialize, Deserialize, Serializer, Deserializer};

impl Serialize for HostFileSystem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Only serialize the root path
        self.root_path.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for HostFileSystem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let root_path = PathBuf::deserialize(deserializer)?;
        Ok(HostFileSystem {
            root_path,
            paths: Arc::new(RwLock::new(HashMap::new())),
            ids: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(RwLock::new(1)),
        })
    }
}

impl HostFileSystem {
    pub fn new(root: &str) -> Self {
        HostFileSystem {
            root_path: PathBuf::from(root),
            paths: Arc::new(RwLock::new(HashMap::new())),
            ids: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(RwLock::new(1)), // 0 is usually root
        }
    }

    fn get_path(&self, qid: &Qid) -> Option<PathBuf> {
        self.paths.read().unwrap().get(&qid.path).cloned()
    }

    fn get_or_create_id(&self, path: &Path) -> u64 {
        let mut ids = self.ids.write().unwrap();
        // Canonicalize? Or assume relative walking keeps it simpler.
        // Let's stick strictly to what we resolve.
        
        if let Some(&id) = ids.get(path) {
            return id;
        }

        let mut next = self.next_id.write().unwrap();
        let id = *next;
        *next += 1;
        
        ids.insert(path.to_path_buf(), id);
        self.paths.write().unwrap().insert(id, path.to_path_buf());
        
        id
    }
    
    // Convert std::fs::Metadata to specific Qid Type
    fn metadata_to_qtype(metadata: &fs::Metadata) -> u8 {
        if metadata.is_dir() { 
            P9_QTDIR 
        } else if metadata.is_symlink() {
            P9_QTSYMLINK
        } else {
            P9_QTFILE
        }
    }
    
    fn metadata_to_attr(metadata: &fs::Metadata, qid: Qid) -> FileAttr {
         let size = metadata.len();
         let blocks = metadata.blocks();
         let blksize = metadata.blksize() as u32;
         let nlink = metadata.nlink();
         let uid = metadata.uid();
         let gid = metadata.gid();
         let mode = metadata.mode();
         
         let atime_sec = metadata.atime();
         let atime_nsec = metadata.atime_nsec() as u64;
         let mtime_sec = metadata.mtime();
         let mtime_nsec = metadata.mtime_nsec() as u64;
         let ctime_sec = metadata.ctime();
         let ctime_nsec = metadata.ctime_nsec() as u64;

         FileAttr {
             qid,
             mode,
             uid,
             gid,
             nlink,
             rdev: 0, // todo
             size,
             blksize,
             blocks,
             atime: (atime_sec as u64, atime_nsec),
             mtime: (mtime_sec as u64, mtime_nsec),
             ctime: (ctime_sec as u64, ctime_nsec),
         }
    }
}

impl FileSystem for HostFileSystem {
    fn attach(&mut self) -> Result<Qid, u32> {
        let root = self.root_path.clone();
        if !root.exists() {
            return Err(2); // ENOENT
        }
        
        // Use 0 as root ID
        let id = 0;
        self.paths.write().unwrap().insert(id, root.clone());
        self.ids.write().unwrap().insert(root.clone(), id);
        
        let metadata = fs::metadata(&root).map_err(|_| 5u32)?; // EIO
        let qtype = Self::metadata_to_qtype(&metadata);
        
        Ok(Qid::new(qtype, id))
    }

    fn walk(&mut self, parent_qid: &Qid, name: &str) -> Result<Qid, u32> {
        let parent_path = self.get_path(parent_qid).ok_or(2u32)?; // ENOENT
        
        let child_path = if name == ".." {
            parent_path.parent().unwrap_or(&parent_path).to_path_buf()
        } else if name == "." {
            parent_path.clone()
        } else {
            parent_path.join(name)
        };
        
        if !child_path.exists() {
            return Err(2); // ENOENT
        }
        
        let metadata = fs::symlink_metadata(&child_path).map_err(|_| 5u32)?;
        let id = self.get_or_create_id(&child_path);
        let qtype = Self::metadata_to_qtype(&metadata);
        
        Ok(Qid::new(qtype, id))
    }

    fn getattr(&mut self, qid: &Qid) -> Result<FileAttr, u32> {
        let path = self.get_path(qid).ok_or(2u32)?;
        // Use symlink_metadata to not follow links automatically (9P handles links)
        let metadata = fs::symlink_metadata(&path).map_err(|_| 2u32)?;
        Ok(Self::metadata_to_attr(&metadata, *qid))
    }

    fn open(&mut self, qid: &Qid, flags: u32) -> Result<(), u32> {
        let path = self.get_path(qid).ok_or(2u32)?;
        if !path.exists() {
            return Err(2);
        }
        // In this simple mapped FS, we don't hold file handles yet. 
        // We open on read/write.
        Ok(())
    }

    fn create(&mut self, parent_qid: &Qid, name: &str, mode: u32, flags: u32) -> Result<Qid, u32> {
        let parent_path = self.get_path(parent_qid).ok_or(2u32)?;
        let child_path = parent_path.join(name);
        
        if child_path.exists() {
            return Err(17); // EEXIST
        }
        
        // Apply mode. Note: 9P mode includes type bits.
        // We need to filter for regular file creation.
        // For now just create empty file.
        
        let f = fs::File::create(&child_path).map_err(|_| 5u32)?; // EIO
        // Set permissions if possible
        let mut perms = f.metadata().unwrap().permissions();
        perms.set_mode(mode & 0o777); 
        fs::set_permissions(&child_path, perms).ok();
        
        let metadata = f.metadata().map_err(|_| 5u32)?;
        let id = self.get_or_create_id(&child_path);
        
        Ok(Qid::new(P9_QTFILE, id))
    }

    fn mkdir(&mut self, parent_qid: &Qid, name: &str, mode: u32) -> Result<Qid, u32> {
        let parent_path = self.get_path(parent_qid).ok_or(2u32)?;
        let child_path = parent_path.join(name);
        
        fs::create_dir(&child_path).map_err(|_| 5u32)?;
        
        let perms = fs::Permissions::from_mode(mode & 0o777);
        fs::set_permissions(&child_path, perms).ok();
        
        let id = self.get_or_create_id(&child_path);
        Ok(Qid::new(P9_QTDIR, id))
    }

    fn read(&mut self, qid: &Qid, offset: u64, count: u32) -> Result<Vec<u8>, u32> {
        let path = self.get_path(qid).ok_or(2u32)?;
        
        let mut file = fs::File::open(&path).map_err(|_| 5u32)?;
        use std::io::{Read, Seek, SeekFrom};
        
        file.seek(SeekFrom::Start(offset)).map_err(|_| 5u32)?;
        
        // Limit read to count or reasonably small buffer
        let to_read = count as usize; // Check max?
        let mut buf = vec![0u8; to_read];
        let bytes_read = file.read(&mut buf).map_err(|_| 5u32)?;
        
        buf.truncate(bytes_read);
        Ok(buf)
    }

    fn write(&mut self, qid: &Qid, offset: u64, data: &[u8]) -> Result<u32, u32> {
        let path = self.get_path(qid).ok_or(2u32)?;
        
        // Open for writing
        use std::fs::OpenOptions;
        use std::io::{Write, Seek, SeekFrom};
        
        let mut file = OpenOptions::new().write(true).open(&path).map_err(|_| 5u32)?;
        
        file.seek(SeekFrom::Start(offset)).map_err(|_| 5u32)?;
        file.write_all(data).map_err(|_| 5u32)?;
        
        Ok(data.len() as u32)
    }

    fn readdir(&mut self, qid: &Qid, offset: u64, count: u32) -> Result<Vec<DirEntry>, u32> {
        let path = self.get_path(qid).ok_or(2u32)?;
        
        let read_dir = fs::read_dir(&path).map_err(|_| 20u32)?; // ENOTDIR?
        
        let mut entries = Vec::new();
        let mut current_pos = 0;
        
        for entry in read_dir {
            if current_pos >= offset {
                let entry = entry.map_err(|_| 5u32)?;
                let entry_path = entry.path();
                let metadata = entry.metadata().map_err(|_| 5u32)?;
                
                let id = self.get_or_create_id(&entry_path);
                
                entries.push(DirEntry {
                    qid: Qid::new(Self::metadata_to_qtype(&metadata), id),
                    offset: current_pos + 1,
                    type_: Self::metadata_to_qtype(&metadata),
                    name: entry.file_name().to_string_lossy().to_string(),
                });
                
                // If we have "enough", we could stop, but for now we rely on the implementation 
                // in mod.rs to filter/serialize.
            }
            current_pos += 1;
        }
        
        Ok(entries)
    }

    fn remove(&mut self, qid: &Qid) -> Result<(), u32> {
        let path = self.get_path(qid).ok_or(2u32)?;
        
        if path.is_dir() {
            fs::remove_dir(&path).map_err(|_| 39) // ENOTEMPTY or other
        } else {
            fs::remove_file(&path).map_err(|_| 5)
        }
    }

    fn rename(&mut self, qid: &Qid, new_dir: &Qid, new_name: &str) -> Result<(), u32> {
        let old_path = self.get_path(qid).ok_or(2u32)?;
        let new_dir_path = self.get_path(new_dir).ok_or(2u32)?;
        let new_path = new_dir_path.join(new_name);
        
        fs::rename(&old_path, &new_path).map_err(|_| 5u32)?;
        
        // Update mappings
        let id = qid.path;
        self.paths.write().unwrap().insert(id, new_path.clone());
        // Remove old mapping from ids? Ideally yes.
        let mut ids = self.ids.write().unwrap();
        ids.remove(&old_path);
        ids.insert(new_path, id);
        
        Ok(())
    }
}
