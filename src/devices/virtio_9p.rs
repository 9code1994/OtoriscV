//! VirtIO-9p filesystem device
//!
//! Implements 9P2000.L protocol over VirtIO transport for filesystem access.
//! This allows the guest to access a filesystem provided by the host (browser).

use super::virtio::{VirtioMmio, Virtqueue, Descriptor};
use std::collections::{HashMap, VecDeque, HashSet};
use crate::memory::Memory;
use serde::{Serialize, Deserialize};

// 9P2000.L message types
const P9_TLERROR: u8 = 6;
const P9_RLERROR: u8 = 7;
const P9_TSTATFS: u8 = 8;
const P9_RSTATFS: u8 = 9;
const P9_TLOPEN: u8 = 12;
const P9_RLOPEN: u8 = 13;
const P9_TLCREATE: u8 = 14;
const P9_RLCREATE: u8 = 15;
const P9_TSYMLINK: u8 = 16;
const P9_RSYMLINK: u8 = 17;
const P9_TMKNOD: u8 = 18;
const P9_RMKNOD: u8 = 19;
const P9_TRENAME: u8 = 20;
const P9_RRENAME: u8 = 21;
const P9_TREADLINK: u8 = 22;
const P9_RREADLINK: u8 = 23;
const P9_TGETATTR: u8 = 24;
const P9_RGETATTR: u8 = 25;
const P9_TSETATTR: u8 = 26;
const P9_RSETATTR: u8 = 27;
const P9_TXATTRWALK: u8 = 30;
const P9_RXATTRWALK: u8 = 31;
const P9_TXATTRCREATE: u8 = 32;
const P9_RXATTRCREATE: u8 = 33;
const P9_TREADDIR: u8 = 40;
const P9_RREADDIR: u8 = 41;
const P9_TFSYNC: u8 = 50;
const P9_RFSYNC: u8 = 51;
const P9_TLOCK: u8 = 52;
const P9_RLOCK: u8 = 53;
const P9_TGETLOCK: u8 = 54;
const P9_RGETLOCK: u8 = 55;
const P9_TLINK: u8 = 70;
const P9_RLINK: u8 = 71;
const P9_TMKDIR: u8 = 72;
const P9_RMKDIR: u8 = 73;
const P9_TRENAMEAT: u8 = 74;
const P9_RRENAMEAT: u8 = 75;
const P9_TUNLINKAT: u8 = 76;
const P9_RUNLINKAT: u8 = 77;
const P9_TVERSION: u8 = 100;
const P9_RVERSION: u8 = 101;
const P9_TAUTH: u8 = 102;
const P9_RAUTH: u8 = 103;
const P9_TATTACH: u8 = 104;
const P9_RATTACH: u8 = 105;
const P9_TFLUSH: u8 = 108;
const P9_RFLUSH: u8 = 109;
const P9_TWALK: u8 = 110;
const P9_RWALK: u8 = 111;
const P9_TREAD: u8 = 116;
const P9_RREAD: u8 = 117;
const P9_TWRITE: u8 = 118;
const P9_RWRITE: u8 = 119;
const P9_TCLUNK: u8 = 120;
const P9_RCLUNK: u8 = 121;

// 9P QID types
const P9_QTDIR: u8 = 0x80;
const P9_QTAPPEND: u8 = 0x40;
const P9_QTEXCL: u8 = 0x20;
const P9_QTMOUNT: u8 = 0x10;
const P9_QTAUTH: u8 = 0x08;
const P9_QTTMP: u8 = 0x04;
const P9_QTSYMLINK: u8 = 0x02;
const P9_QTLINK: u8 = 0x01;
const P9_QTFILE: u8 = 0x00;

// Error codes
const ENOENT: u32 = 2;
const EIO: u32 = 5;
const EBADF: u32 = 9;
const ENOMEM: u32 = 12;
const ENOTDIR: u32 = 20;
const EISDIR: u32 = 21;
const EINVAL: u32 = 22;
const ENOSPC: u32 = 28;
const ENOTEMPTY: u32 = 39;

/// A 9P QID (unique identifier for a file)
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Qid {
    pub qtype: u8,
    pub version: u32,
    pub path: u64,
}

impl Qid {
    pub fn new(qtype: u8, path: u64) -> Self {
        Qid { qtype, version: 0, path }
    }
    
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(13);
        buf.push(self.qtype);
        buf.extend_from_slice(&self.version.to_le_bytes());
        buf.extend_from_slice(&self.path.to_le_bytes());
        buf
    }
}

/// Inode representing a file or directory
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
    /// For files: content hash for lazy loading, or inline data
    pub content: InodeContent,
    /// For directories: child inode paths
    pub children: Vec<u64>,
    /// Parent inode path (0 for root)
    pub parent: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum InodeContent {
    /// Content stored inline (for small files)
    Inline(Vec<u8>),
    /// Content identified by hash (for lazy loading)
    Hash(String),
    /// Directory (no content)
    Directory,
    /// Symlink target
    Symlink(String),
}

impl Inode {
    pub fn is_dir(&self) -> bool {
        (self.mode & 0o170000) == 0o040000
    }
    
    pub fn is_file(&self) -> bool {
        (self.mode & 0o170000) == 0o100000
    }
    
    pub fn is_symlink(&self) -> bool {
        (self.mode & 0o170000) == 0o120000
    }
}

/// 9P Fid - represents an open file handle
#[derive(Clone, Serialize, Deserialize)]
pub struct Fid {
    pub inode_path: u64,
    pub open: bool,
    pub open_flags: u32,
    pub position: u64,
}

/// VirtIO-9p device
#[derive(Serialize, Deserialize)]
pub struct Virtio9p {
    /// VirtIO MMIO base
    pub virtio: VirtioMmio,
    /// Filesystem tag (mount point identifier)
    pub tag: String,
    /// Inode table
    pub inodes: HashMap<u64, Inode>,
    /// Next inode path
    pub next_inode: u64,
    /// Active fids
    pub fids: HashMap<u32, Fid>,
    /// Maximum message size
    pub msize: u32,
    /// Pending requests
    pub pending_requests: Vec<Vec<u8>>,
    /// Pending responses
    /// Pending responses
    pub pending_responses: VecDeque<Vec<u8>>,
    
    // Lazy loading
    pub blob_cache: HashMap<String, Vec<u8>>,
    pub missing_blobs: HashSet<String>,
    pub suspended_requests: Vec<SuspendedRequest>,
}

#[derive(Serialize, Deserialize)]
pub struct SuspendedRequest {
    pub queue_idx: usize,
    pub head_idx: u16,
    pub output_descriptors: Vec<Descriptor>,
    pub tag: u16,
    pub input_buffer: Vec<u8>,
}

impl Virtio9p {
    pub fn new(tag: &str) -> Self {
        // Config contains the tag length and tag string
        let mut config = Vec::new();
        let tag_bytes = tag.as_bytes();
        config.extend_from_slice(&(tag_bytes.len() as u16).to_le_bytes());
        config.extend_from_slice(tag_bytes);
        
        // Pad to at least 8 bytes
        while config.len() < 8 {
            config.push(0);
        }
        
        let mut virtio = VirtioMmio::new(9, 1, config); // Device ID 9 = 9p
        
        // Set device features
        // VIRTIO_9P_MOUNT_TAG
        virtio.device_features = 1;
        
        let mut device = Virtio9p {
            virtio,
            tag: tag.to_string(),
            inodes: HashMap::new(),
            next_inode: 1,
            fids: HashMap::new(),
            msize: 8192,
            pending_requests: Vec::new(),
            pending_responses: VecDeque::new(),
            blob_cache: HashMap::new(),
            missing_blobs: HashSet::new(),
            suspended_requests: Vec::new(),
        };
        
        // Create root inode
        let root_qid = Qid::new(P9_QTDIR, 0);
        let root = Inode {
            qid: root_qid,
            name: String::new(),
            size: 0,
            mode: 0o40755, // Directory with rwxr-xr-x
            uid: 0,
            gid: 0,
            mtime: 0,
            atime: 0,
            ctime: 0,
            content: InodeContent::Directory,
            children: Vec::new(),
            parent: 0,
        };
        device.inodes.insert(0, root);
        
        device
    }
    
    pub fn read8(&self, offset: u32) -> u8 {
        self.virtio.read8(offset)
    }
    
    pub fn write8(&mut self, offset: u32, value: u8) {
        self.virtio.write8(offset, value);
    }
    
    pub fn read32(&self, offset: u32) -> u32 {
        self.virtio.read32(offset)
    }
    
    pub fn write32(&mut self, offset: u32, value: u32) {
        self.virtio.write32(offset, value);
    }
    
    /// Process pending queues
    pub fn process_queues(&mut self, mem: &mut Memory) {
        // Collect pending queues to avoid borrow issues if we iterated queue_notify_pending directly?
        // Actually .pop_front() removes item, so we don't hold borrow on vector.
        // But we need to borrow `self.virtio` to pop.
        let mut queues_to_process = Vec::new();
        while let Some(q) = self.virtio.queue_notify_pending.pop_front() {
            queues_to_process.push(q);
        }
        
        // Ensure we process each queue only once per batch (deduplicate)
        queues_to_process.sort_unstable();
        queues_to_process.dedup();
        
        for queue_idx in queues_to_process {
            self.process_queue(mem, queue_idx as usize);
        }
    }

    fn process_queue(&mut self, mem: &mut Memory, queue_idx: usize) {
        let mut processed_any = false;
        
        loop {
            // STEP 1: Borrow queue to check availability and read input
            // We use a block to limit the borrow scope of `self.virtio`
            let (head_idx, input_buffer, output_descriptors) = {
                let queue = if let Some(q) = self.virtio.queues.get_mut(queue_idx) {
                    q
                } else {
                    return;
                };
                
                if !queue.ready {
                    return;
                }
                
                let avail_idx = queue.avail_idx(mem);
                if queue.last_avail_idx == avail_idx {
                    break;
                }
                
                let head_idx = queue.get_avail_head(mem, queue.last_avail_idx);
                queue.last_avail_idx = queue.last_avail_idx.wrapping_add(1);
                
                let mut desc_idx = head_idx;
                let mut input = Vec::new();
                let mut output = Vec::new();
                
                loop {
                    let desc = queue.read_desc(mem, desc_idx);
                    if (desc.flags & super::virtio::VRING_DESC_F_WRITE) == 0 {
                         for i in 0..desc.len {
                             input.push(mem.read8((desc.addr + i as u64) as u32));
                         }
                    } else {
                        output.push(desc);
                    }
                    
                    if (desc.flags & super::virtio::VRING_DESC_F_NEXT) == 0 {
                        break;
                    }
                    desc_idx = desc.next;
                }
                (head_idx, input, output)
            };
            
            // STEP 2: Process message (No borrow of virtio here, only self methods that use other fields)
            let result = self.process_message(&input_buffer);
            
            match result {
                Some(response) => {
                    // STEP 3: Write response and update used ring
                    {
                        // Re-borrow queue from self.virtio
                        let queue = &mut self.virtio.queues[queue_idx];
                        
                        let mut bytes_written = 0;
                        let mut resp_offset = 0;
                        
                        for desc in output_descriptors {
                            if resp_offset >= response.len() { break; }
                            let to_write = std::cmp::min(desc.len as usize, response.len() - resp_offset);
                            
                            for i in 0..to_write {
                                mem.write32(desc.addr as u32 + i as u32, response[resp_offset + i] as u32);
                                mem.write8((desc.addr + i as u64) as u32, response[resp_offset + i]);
                            }
                            resp_offset += to_write;
                            bytes_written += to_write;
                        }
                        
                        queue.push_used(mem, head_idx as u32, bytes_written as u32);
                    }
                    processed_any = true;
                },
                None => {
                    // Suspended request (waiting for lazy load)
                    // We need to extract the tag from the input buffer to identify the request later?
                    // 9P header: size[4] type[1] tag[2]
                    let tag = if input_buffer.len() >= 7 {
                        u16::from_le_bytes([input_buffer[5], input_buffer[6]])
                    } else {
                        0xFFFF // Should not happen for valid requests
                    };
                    
                    self.suspended_requests.push(SuspendedRequest {
                        queue_idx,
                        head_idx,
                        output_descriptors,
                        tag,
                        input_buffer: input_buffer.to_vec(),
                    });
                    
                    // We do NOT push to used ring yet.
                    // Interrupt is optional here? Usually not raised until completion.
                }
            }
        }
        
        if processed_any {
            self.virtio.raise_interrupt(true);
        }
    }
    
    // Kept for compatibility if interface requires it, but empty
    pub fn notify(&mut self, _queue: u32) {}
    
    /// Process a 9P request message and return a response
    pub fn process_message(&mut self, request: &[u8]) -> Option<Vec<u8>> {
        if request.len() < 7 {
            return Some(self.error_response(0, EINVAL));
        }
        
        let msg_type = request[4];
        let tag = u16::from_le_bytes([request[5], request[6]]);
        let payload = &request[7..];
        
        match msg_type {
            P9_TVERSION => Some(self.handle_version(tag, payload)),
            P9_TATTACH => Some(self.handle_attach(tag, payload)),
            P9_TWALK => Some(self.handle_walk(tag, payload)),
            P9_TCLUNK => Some(self.handle_clunk(tag, payload)),
            P9_TGETATTR => Some(self.handle_getattr(tag, payload)),
            P9_TREADDIR => Some(self.handle_readdir(tag, payload)),
            P9_TLOPEN => Some(self.handle_lopen(tag, payload)),
            P9_TREAD => self.handle_read(tag, payload), // Returns Option
            P9_TSTATFS => Some(self.handle_statfs(tag, payload)),
            _ => Some(self.error_response(tag, EINVAL)),
        }
    }
    
    fn error_response(&self, tag: u16, errno: u32) -> Vec<u8> {
        let mut resp = Vec::new();
        resp.extend_from_slice(&0u32.to_le_bytes()); // Size placeholder
        resp.push(P9_RLERROR);
        resp.extend_from_slice(&tag.to_le_bytes());
        resp.extend_from_slice(&errno.to_le_bytes());
        
        let size = resp.len() as u32;
        resp[0..4].copy_from_slice(&size.to_le_bytes());
        resp
    }
    
    fn handle_version(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 6 {
            return self.error_response(tag, EINVAL);
        }
        
        let msize = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let version_len = u16::from_le_bytes([payload[4], payload[5]]) as usize;
        
        if payload.len() < 6 + version_len {
            return self.error_response(tag, EINVAL);
        }
        
        // Use smaller of requested and our max
        self.msize = msize.min(8192);
        
        // Build response
        let version = b"9P2000.L";
        let mut resp = Vec::new();
        resp.extend_from_slice(&0u32.to_le_bytes()); // Size placeholder
        resp.push(P9_RVERSION);
        resp.extend_from_slice(&tag.to_le_bytes());
        resp.extend_from_slice(&self.msize.to_le_bytes());
        resp.extend_from_slice(&(version.len() as u16).to_le_bytes());
        resp.extend_from_slice(version);
        
        let size = resp.len() as u32;
        resp[0..4].copy_from_slice(&size.to_le_bytes());
        resp
    }
    
    fn handle_attach(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 12 {
            return self.error_response(tag, EINVAL);
        }
        
        let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let _afid = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
        
        // Create fid pointing to root
        self.fids.insert(fid, Fid {
            inode_path: 0,
            open: false,
            open_flags: 0,
            position: 0,
        });
        
        // Get root QID
        let root = self.inodes.get(&0).unwrap();
        let qid = root.qid.encode();
        
        let mut resp = Vec::new();
        resp.extend_from_slice(&0u32.to_le_bytes());
        resp.push(P9_RATTACH);
        resp.extend_from_slice(&tag.to_le_bytes());
        resp.extend_from_slice(&qid);
        
        let size = resp.len() as u32;
        resp[0..4].copy_from_slice(&size.to_le_bytes());
        resp
    }
    
    fn handle_walk(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 10 {
            return self.error_response(tag, EINVAL);
        }
        
        let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let newfid = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
        let nwname = u16::from_le_bytes([payload[8], payload[9]]) as usize;
        
        // Get starting inode
        let start_fid = match self.fids.get(&fid) {
            Some(f) => f.clone(),
            None => return self.error_response(tag, EBADF),
        };
        
        let mut current_path = start_fid.inode_path;
        let mut qids = Vec::new();
        
        // Parse and walk path components
        let mut offset = 10;
        for _ in 0..nwname {
            if offset + 2 > payload.len() {
                return self.error_response(tag, EINVAL);
            }
            
            let name_len = u16::from_le_bytes([payload[offset], payload[offset + 1]]) as usize;
            offset += 2;
            
            if offset + name_len > payload.len() {
                return self.error_response(tag, EINVAL);
            }
            
            let name = String::from_utf8_lossy(&payload[offset..offset + name_len]).to_string();
            offset += name_len;
            
            // Look up in current directory
            let current = match self.inodes.get(&current_path) {
                Some(i) => i,
                None => return self.error_response(tag, ENOENT),
            };
            
            if !current.is_dir() {
                return self.error_response(tag, ENOTDIR);
            }
            
            // Handle special names
            if name == "." {
                qids.push(current.qid.encode());
                continue;
            } else if name == ".." {
                current_path = current.parent;
                let parent = self.inodes.get(&current_path).unwrap();
                qids.push(parent.qid.encode());
                continue;
            }
            
            // Find child by name
            let mut found = false;
            for &child_path in &current.children {
                if let Some(child) = self.inodes.get(&child_path) {
                    if child.name == name {
                        current_path = child_path;
                        qids.push(child.qid.encode());
                        found = true;
                        break;
                    }
                }
            }
            
            if !found {
                if qids.is_empty() {
                    return self.error_response(tag, ENOENT);
                }
                // Partial walk - return what we have
                break;
            }
        }
        
        // Create new fid
        self.fids.insert(newfid, Fid {
            inode_path: current_path,
            open: false,
            open_flags: 0,
            position: 0,
        });
        
        // Build response
        let mut resp = Vec::new();
        resp.extend_from_slice(&0u32.to_le_bytes());
        resp.push(P9_RWALK);
        resp.extend_from_slice(&tag.to_le_bytes());
        resp.extend_from_slice(&(qids.len() as u16).to_le_bytes());
        for qid in qids {
            resp.extend_from_slice(&qid);
        }
        
        let size = resp.len() as u32;
        resp[0..4].copy_from_slice(&size.to_le_bytes());
        resp
    }
    
    fn handle_clunk(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 4 {
            return self.error_response(tag, EINVAL);
        }
        
        let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        self.fids.remove(&fid);
        
        let mut resp = Vec::new();
        resp.extend_from_slice(&0u32.to_le_bytes());
        resp.push(P9_RCLUNK);
        resp.extend_from_slice(&tag.to_le_bytes());
        
        let size = resp.len() as u32;
        resp[0..4].copy_from_slice(&size.to_le_bytes());
        resp
    }
    
    fn handle_getattr(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 12 {
            return self.error_response(tag, EINVAL);
        }
        
        let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let _request_mask = u64::from_le_bytes([
            payload[4], payload[5], payload[6], payload[7],
            payload[8], payload[9], payload[10], payload[11],
        ]);
        
        let f = match self.fids.get(&fid) {
            Some(f) => f,
            None => return self.error_response(tag, EBADF),
        };
        
        let inode = match self.inodes.get(&f.inode_path) {
            Some(i) => i,
            None => return self.error_response(tag, ENOENT),
        };
        
        let mut resp = Vec::new();
        resp.extend_from_slice(&0u32.to_le_bytes());
        resp.push(P9_RGETATTR);
        resp.extend_from_slice(&tag.to_le_bytes());
        
        // Valid mask (what we're returning)
        let valid: u64 = 0x7fff; // All basic attrs
        resp.extend_from_slice(&valid.to_le_bytes());
        
        // QID
        resp.extend_from_slice(&inode.qid.encode());
        
        // Mode, uid, gid
        resp.extend_from_slice(&inode.mode.to_le_bytes());
        resp.extend_from_slice(&inode.uid.to_le_bytes());
        resp.extend_from_slice(&inode.gid.to_le_bytes());
        
        // nlink
        resp.extend_from_slice(&1u64.to_le_bytes());
        
        // rdev
        resp.extend_from_slice(&0u64.to_le_bytes());
        
        // size
        resp.extend_from_slice(&inode.size.to_le_bytes());
        
        // blksize
        resp.extend_from_slice(&4096u64.to_le_bytes());
        
        // blocks
        let blocks = (inode.size + 511) / 512;
        resp.extend_from_slice(&blocks.to_le_bytes());
        
        // atime_sec, atime_nsec
        resp.extend_from_slice(&inode.atime.to_le_bytes());
        resp.extend_from_slice(&0u64.to_le_bytes());
        
        // mtime_sec, mtime_nsec
        resp.extend_from_slice(&inode.mtime.to_le_bytes());
        resp.extend_from_slice(&0u64.to_le_bytes());
        
        // ctime_sec, ctime_nsec
        resp.extend_from_slice(&inode.ctime.to_le_bytes());
        resp.extend_from_slice(&0u64.to_le_bytes());
        
        // btime_sec, btime_nsec (birth time)
        resp.extend_from_slice(&0u64.to_le_bytes());
        resp.extend_from_slice(&0u64.to_le_bytes());
        
        // gen, data_version
        resp.extend_from_slice(&0u64.to_le_bytes());
        resp.extend_from_slice(&0u64.to_le_bytes());
        
        let size = resp.len() as u32;
        resp[0..4].copy_from_slice(&size.to_le_bytes());
        resp
    }
    
    fn handle_readdir(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 16 {
            return self.error_response(tag, EINVAL);
        }
        
        let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let offset = u64::from_le_bytes([
            payload[4], payload[5], payload[6], payload[7],
            payload[8], payload[9], payload[10], payload[11],
        ]);
        let count = u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
        
        let f = match self.fids.get(&fid) {
            Some(f) => f,
            None => return self.error_response(tag, EBADF),
        };
        
        let inode = match self.inodes.get(&f.inode_path) {
            Some(i) => i.clone(),
            None => return self.error_response(tag, ENOENT),
        };
        
        if !inode.is_dir() {
            return self.error_response(tag, ENOTDIR);
        }
        
        // Build directory entries
        let mut entries = Vec::new();
        let mut current_offset = 0u64;
        
        for &child_path in &inode.children {
            if current_offset < offset {
                current_offset += 1;
                continue;
            }
            
            if let Some(child) = self.inodes.get(&child_path) {
                let mut entry = Vec::new();
                
                // QID
                entry.extend_from_slice(&child.qid.encode());
                
                // Offset (next entry)
                entry.extend_from_slice(&(current_offset + 1).to_le_bytes());
                
                // Type
                entry.push(child.qid.qtype);
                
                // Name
                let name_bytes = child.name.as_bytes();
                entry.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
                entry.extend_from_slice(name_bytes);
                
                if entries.len() + entry.len() > count as usize {
                    break;
                }
                
                entries.extend_from_slice(&entry);
            }
            
            current_offset += 1;
        }
        
        let mut resp = Vec::new();
        resp.extend_from_slice(&0u32.to_le_bytes());
        resp.push(P9_RREADDIR);
        resp.extend_from_slice(&tag.to_le_bytes());
        resp.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        resp.extend_from_slice(&entries);
        
        let size = resp.len() as u32;
        resp[0..4].copy_from_slice(&size.to_le_bytes());
        resp
    }
    
    fn handle_lopen(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 8 {
            return self.error_response(tag, EINVAL);
        }
        
        let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let flags = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
        
        let f = match self.fids.get_mut(&fid) {
            Some(f) => f,
            None => return self.error_response(tag, EBADF),
        };
        
        let inode = match self.inodes.get(&f.inode_path) {
            Some(i) => i,
            None => return self.error_response(tag, ENOENT),
        };
        
        f.open = true;
        f.open_flags = flags;
        f.position = 0;
        
        let mut resp = Vec::new();
        resp.extend_from_slice(&0u32.to_le_bytes());
        resp.push(P9_RLOPEN);
        resp.extend_from_slice(&tag.to_le_bytes());
        resp.extend_from_slice(&inode.qid.encode());
        resp.extend_from_slice(&4096u32.to_le_bytes()); // iounit
        
        let size = resp.len() as u32;
        resp[0..4].copy_from_slice(&size.to_le_bytes());
        resp
    }
    
    pub fn get_missing_blobs(&self) -> Vec<String> {
        self.missing_blobs.iter().cloned().collect()
    }
    
    pub fn provide_blob(&mut self, hash: String, data: Vec<u8>, mem: &mut Memory) {
        self.blob_cache.insert(hash.clone(), data);
        self.missing_blobs.remove(&hash);
        
        // Retry suspended requests
        // Take ownership of requests to avoid borrowing self
        let requests = std::mem::replace(&mut self.suspended_requests, Vec::new());
        let mut still_suspended = Vec::new();
        let mut processing_occurred = false;
        
        for req in requests {
            if let Some(response) = self.process_message(&req.input_buffer) {
                // Completed! Write back
                {
                     let queue = &mut self.virtio.queues[req.queue_idx];
                     let mut bytes_written = 0;
                     let mut resp_offset = 0;
                     
                     for desc in &req.output_descriptors {
                         if resp_offset >= response.len() { break; }
                         let to_write = std::cmp::min(desc.len as usize, response.len() - resp_offset);
                         
                         for k in 0..to_write {
                             mem.write32(desc.addr as u32 + k as u32, response[resp_offset + k] as u32);
                             mem.write8((desc.addr + k as u64) as u32, response[resp_offset + k]);
                         }
                         resp_offset += to_write;
                         bytes_written += to_write;
                     }
                     
                     queue.push_used(mem, req.head_idx as u32, bytes_written as u32);
                }
                processing_occurred = true;
            } else {
                still_suspended.push(req);
            }
        }
        
        // Put back still suspended requests
        self.suspended_requests.extend(still_suspended);
        
        if processing_occurred {
             self.virtio.raise_interrupt(true);
        }
    }
    
    fn handle_read(&mut self, tag: u16, payload: &[u8]) -> Option<Vec<u8>> {
        if payload.len() < 16 {
            return Some(self.error_response(tag, EINVAL));
        }
        
        let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let offset = u64::from_le_bytes([
            payload[4], payload[5], payload[6], payload[7],
            payload[8], payload[9], payload[10], payload[11],
        ]);
        let count = u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
        
        let f = match self.fids.get(&fid) {
            Some(f) => f,
            None => return Some(self.error_response(tag, EBADF)),
        };
        
        let inode = match self.inodes.get(&f.inode_path) {
            Some(i) => i.clone(),
            None => return Some(self.error_response(tag, ENOENT)),
        };
        
        // Get data based on content type
        let data = match &inode.content {
            InodeContent::Inline(data) => {
                let start = offset as usize;
                let end = (offset as usize + count as usize).min(data.len());
                if start >= data.len() {
                    Vec::new()
                } else {
                    data[start..end].to_vec()
                }
            }
            InodeContent::Hash(hash) => {
                if let Some(blob) = self.blob_cache.get(hash) {
                    let start = offset as usize;
                    let end = (offset as usize + count as usize).min(blob.len());
                    if start >= blob.len() {
                        Vec::new()
                    } else {
                        blob[start..end].to_vec()
                    }
                } else {
                    // Blob missing, trigger load
                    self.missing_blobs.insert(hash.clone());
                    // Return None to suspend request
                    return None;
                }
            }
            InodeContent::Symlink(target) => {
                let bytes = target.as_bytes();
                let start = offset as usize;
                let end = (offset as usize + count as usize).min(bytes.len());
                if start >= bytes.len() {
                    Vec::new()
                } else {
                    bytes[start..end].to_vec()
                }
            }
            InodeContent::Directory => {
                return Some(self.error_response(tag, EISDIR));
            }
        };
        
        let mut resp = Vec::new();
        resp.extend_from_slice(&0u32.to_le_bytes());
        resp.push(P9_RREAD);
        resp.extend_from_slice(&tag.to_le_bytes());
        resp.extend_from_slice(&(data.len() as u32).to_le_bytes());
        resp.extend_from_slice(&data);
        
        let size = resp.len() as u32;
        resp[0..4].copy_from_slice(&size.to_le_bytes());
        Some(resp)
    }
    
    fn handle_statfs(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 4 {
            return self.error_response(tag, EINVAL);
        }
        
        let mut resp = Vec::new();
        resp.extend_from_slice(&0u32.to_le_bytes());
        resp.push(P9_RSTATFS);
        resp.extend_from_slice(&tag.to_le_bytes());
        
        // type
        resp.extend_from_slice(&0x01021997u32.to_le_bytes()); // V9FS_MAGIC
        // bsize
        resp.extend_from_slice(&4096u32.to_le_bytes());
        // blocks
        resp.extend_from_slice(&1000000u64.to_le_bytes());
        // bfree
        resp.extend_from_slice(&500000u64.to_le_bytes());
        // bavail
        resp.extend_from_slice(&500000u64.to_le_bytes());
        // files
        resp.extend_from_slice(&100000u64.to_le_bytes());
        // ffree
        resp.extend_from_slice(&50000u64.to_le_bytes());
        // fsid
        resp.extend_from_slice(&0u64.to_le_bytes());
        // namelen
        resp.extend_from_slice(&255u32.to_le_bytes());
        
        let size = resp.len() as u32;
        resp[0..4].copy_from_slice(&size.to_le_bytes());
        resp
    }
    
    /// Add a file to the filesystem
    pub fn add_file(&mut self, parent_path: u64, name: &str, content: InodeContent, mode: u32) -> u64 {
        let path = self.next_inode;
        self.next_inode += 1;
        
        let qtype = match &content {
            InodeContent::Directory => P9_QTDIR,
            InodeContent::Symlink(_) => P9_QTSYMLINK,
            _ => P9_QTFILE,
        };
        
        let size = match &content {
            InodeContent::Inline(data) => data.len() as u64,
            InodeContent::Symlink(target) => target.len() as u64,
            _ => 0,
        };
        
        let inode = Inode {
            qid: Qid::new(qtype, path),
            name: name.to_string(),
            size,
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
        
        self.inodes.insert(path, inode);
        
        // Add to parent's children
        if let Some(parent) = self.inodes.get_mut(&parent_path) {
            parent.children.push(path);
        }
        
        path
    }
    
    /// Create a directory
    pub fn mkdir(&mut self, parent_path: u64, name: &str) -> u64 {
        self.add_file(parent_path, name, InodeContent::Directory, 0o40755)
    }
    
    pub fn reset(&mut self) {
        self.virtio.reset();
        self.fids.clear();
        self.blob_cache.clear();
        self.missing_blobs.clear();
        self.suspended_requests.clear();
    }
    
    pub fn has_interrupt(&self) -> bool {
        self.virtio.interrupt_pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_negotiation() {
        let mut device = Virtio9p::new("test");
        
        // P9_TVERSION: size[4] Tversion[1] tag[2] msize[4] version[s]
        // version string: "9P2000.L" (8 bytes)
        // total size: 4 + 1 + 2 + 4 + 2 + 8 = 21
        let mut request = Vec::new();
        request.extend_from_slice(&(21u32).to_le_bytes()); // size
        request.push(P9_TVERSION); // type
        request.extend_from_slice(&(0u16).to_le_bytes()); // tag
        request.extend_from_slice(&(8192u32).to_le_bytes()); // msize
        request.extend_from_slice(&(8u16).to_le_bytes()); // version len
        request.extend_from_slice(b"9P2000.L"); // version string
        
        let response = device.process_message(&request);
        
        assert!(response.is_some());
        let response = response.unwrap();
        
        let resp_size = u32::from_le_bytes([response[0], response[1], response[2], response[3]]);
        assert_eq!(resp_size as usize, response.len());
        assert_eq!(response[4], P9_RVERSION);
    }
    
    #[test]
    fn test_lazy_read() {
        let mut device = Virtio9p::new("test");
        let root = 1; // Root inode is always 1
        
        // Add a lazy file
        let file_inode = device.add_file(root, "lazy.txt", InodeContent::Hash("hash123".to_string()), 0o100644);
        
        // Open the file (simulated, we need a valid FID)
        let fid = 100;
        device.fids.insert(fid, Fid {
            inode_path: file_inode,
            open: true,
            open_flags: 0,
            position: 0,
        });
        
        // Construct TREAD request
        // P9_TREAD: size[4] Tread[1] tag[2] fid[4] offset[8] count[4]
        // size = 4+1+2+4+8+4 = 23
        let mut request = Vec::new();
        let tag = 1;
        request.extend_from_slice(&(23u32).to_le_bytes()); 
        request.push(P9_TREAD);
        request.extend_from_slice(&(tag as u16).to_le_bytes());
        request.extend_from_slice(&(fid as u32).to_le_bytes());
        request.extend_from_slice(&(0u64).to_le_bytes()); // offset
        request.extend_from_slice(&(100u32).to_le_bytes()); // count
        
        // 1. Initial read should be pending (return None)
        let response = device.process_message(&request);
        assert!(response.is_none());
        
        // Check missing blobs
        assert!(device.missing_blobs.contains("hash123"));
        
        // 2. Provide blob (simulating cache update directly to avoid full memory mock)
        device.blob_cache.insert("hash123".to_string(), b"Hello Lazy".to_vec());
        device.missing_blobs.remove("hash123");
        
        // 3. Retry read (should succeed)
        let response = device.process_message(&request);
        assert!(response.is_some());
        let data = response.unwrap();
        
        // Check response type is RREAD
        assert_eq!(data[4], P9_RREAD);
        // Check data content (header: size[4] id[1] tag[2] count[4] data[...])
        // count is at offset 7
        let count = u32::from_le_bytes([data[7], data[8], data[9], data[10]]);
        assert_eq!(count, 10);
        let content = &data[11..];
        assert_eq!(content, b"Hello Lazy");
    }
}
