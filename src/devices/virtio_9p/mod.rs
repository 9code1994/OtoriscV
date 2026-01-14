//! VirtIO-9p filesystem device
//!
//! Implements 9P2000.L protocol over VirtIO transport for filesystem access.

use super::virtio::{VirtioMmio, Virtqueue, Descriptor};
use std::collections::{HashMap, VecDeque};
use crate::memory::Memory;
use serde::{Serialize, Deserialize};

pub mod filesystem;
pub mod in_memory;
#[cfg(not(target_arch = "wasm32"))]
pub mod host;

use filesystem::{FileSystem, FileAttr, DirEntry};

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
pub const P9_QTDIR: u8 = 0x80;
pub const P9_QTAPPEND: u8 = 0x40;
pub const P9_QTEXCL: u8 = 0x20;
pub const P9_QTMOUNT: u8 = 0x10;
pub const P9_QTAUTH: u8 = 0x08;
pub const P9_QTTMP: u8 = 0x04;
pub const P9_QTSYMLINK: u8 = 0x02;
pub const P9_QTLINK: u8 = 0x01;
pub const P9_QTFILE: u8 = 0x00;

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

/// 9P Fid - represents an open file handle
#[derive(Clone, Serialize, Deserialize)]
pub struct Fid {
    pub qid: Qid,
    pub open: bool,
    pub open_flags: u32,
    pub position: u64,
}

#[derive(Serialize, Deserialize)]
pub enum Backend {
    InMemory(in_memory::InMemoryFileSystem),
    #[cfg(not(target_arch = "wasm32"))]
    #[serde(skip)]
    Host(host::HostFileSystem),
}

impl FileSystem for Backend {
    fn attach(&mut self) -> Result<Qid, u32> {
        match self {
            Backend::InMemory(fs) => fs.attach(),
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Host(fs) => fs.attach(),
        }
    }
    fn walk(&mut self, parent_qid: &Qid, name: &str) -> Result<Qid, u32> {
        match self {
             Backend::InMemory(fs) => fs.walk(parent_qid, name),
             #[cfg(not(target_arch = "wasm32"))]
             Backend::Host(fs) => fs.walk(parent_qid, name),
        }
    }
    fn getattr(&mut self, qid: &Qid) -> Result<FileAttr, u32> {
         match self {
             Backend::InMemory(fs) => fs.getattr(qid),
             #[cfg(not(target_arch = "wasm32"))]
             Backend::Host(fs) => fs.getattr(qid),
        }
    }
    fn open(&mut self, qid: &Qid, flags: u32) -> Result<(), u32> {
         match self {
             Backend::InMemory(fs) => fs.open(qid, flags),
             #[cfg(not(target_arch = "wasm32"))]
             Backend::Host(fs) => fs.open(qid, flags),
        }
    }
    fn create(&mut self, parent_qid: &Qid, name: &str, mode: u32, flags: u32) -> Result<Qid, u32> {
         match self {
             Backend::InMemory(fs) => fs.create(parent_qid, name, mode, flags),
             #[cfg(not(target_arch = "wasm32"))]
             Backend::Host(fs) => fs.create(parent_qid, name, mode, flags),
        }
    }
    fn mkdir(&mut self, parent_qid: &Qid, name: &str, mode: u32) -> Result<Qid, u32> {
         match self {
             Backend::InMemory(fs) => fs.mkdir(parent_qid, name, mode),
             #[cfg(not(target_arch = "wasm32"))]
             Backend::Host(fs) => fs.mkdir(parent_qid, name, mode),
        }
    }
    fn read(&mut self, qid: &Qid, offset: u64, count: u32) -> Result<Vec<u8>, u32> {
         match self {
             Backend::InMemory(fs) => fs.read(qid, offset, count),
             #[cfg(not(target_arch = "wasm32"))]
             Backend::Host(fs) => fs.read(qid, offset, count),
        }
    }
    fn write(&mut self, qid: &Qid, offset: u64, data: &[u8]) -> Result<u32, u32> {
         match self {
             Backend::InMemory(fs) => fs.write(qid, offset, data),
             #[cfg(not(target_arch = "wasm32"))]
             Backend::Host(fs) => fs.write(qid, offset, data),
        }
    }
    fn readdir(&mut self, qid: &Qid, offset: u64, count: u32) -> Result<Vec<DirEntry>, u32> {
         match self {
             Backend::InMemory(fs) => fs.readdir(qid, offset, count),
             #[cfg(not(target_arch = "wasm32"))]
             Backend::Host(fs) => fs.readdir(qid, offset, count),
        }
    }
    fn remove(&mut self, qid: &Qid) -> Result<(), u32> {
         match self {
             Backend::InMemory(fs) => fs.remove(qid),
             #[cfg(not(target_arch = "wasm32"))]
             Backend::Host(fs) => fs.remove(qid),
        }
    }
    fn rename(&mut self, qid: &Qid, new_dir: &Qid, new_name: &str) -> Result<(), u32> {
         match self {
             Backend::InMemory(fs) => fs.rename(qid, new_dir, new_name),
             #[cfg(not(target_arch = "wasm32"))]
             Backend::Host(fs) => fs.rename(qid, new_dir, new_name),
        }
    }
}

/// VirtIO-9p device
#[derive(Serialize, Deserialize)]
pub struct Virtio9p {
    /// VirtIO MMIO base
    pub virtio: VirtioMmio,
    /// Filesystem tag (mount point identifier)
    pub tag: String,
    
    /// Filesystem backend
    pub fs: Backend,

    /// Active fids
    pub fids: HashMap<u32, Fid>,
    /// Maximum message size
    pub msize: u32,
    /// Pending requests
    pub pending_requests: Vec<Vec<u8>>,
    /// Pending responses
    pub pending_responses: VecDeque<Vec<u8>>,
    
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
    pub fn new(tag: &str, backend: Backend) -> Self {
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
        
        Virtio9p {
            virtio,
            tag: tag.to_string(),
            fs: backend,
            fids: HashMap::new(),
            msize: 8192,
            pending_requests: Vec::new(),
            pending_responses: VecDeque::new(),
            suspended_requests: Vec::new(),
        }
    }
    
    // ... [Basic virtio methods: read8, write8, read32, write32] ...
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

    pub fn reset(&mut self) {
        // Reset state
        self.fids.clear();
        self.pending_requests.clear();
        self.pending_responses.clear();
        // We reuse FS state normally, but maybe we should re-attach?
        // FS state might be persistent.
    }

    // Lazy load/blob methods - delegate to InMemory or error
    pub fn get_missing_blobs(&self) -> Vec<String> {
        match &self.fs {
            Backend::InMemory(fs) => fs.missing_blobs.iter().cloned().collect(),
            _ => Vec::new(),
        }
    }
    
    pub fn provide_blob(&mut self, hash: String, data: Vec<u8>, mem: &mut Memory) {
        match &mut self.fs {
            Backend::InMemory(fs) => {
                fs.blob_cache.insert(hash.clone(), data);
                fs.missing_blobs.remove(&hash);
                // Retry suspended requests logic would go here
                // For now, simpler implementation: next read will succeed
            },
            _ => {},
        }
    }

    pub fn process_queues(&mut self, mem: &mut Memory) {
        let mut queues_to_process = Vec::new();
        while let Some(q) = self.virtio.queue_notify_pending.pop_front() {
            queues_to_process.push(q);
        }
        queues_to_process.sort_unstable();
        queues_to_process.dedup();
        
        for queue_idx in queues_to_process {
            self.process_queue(mem, queue_idx as usize);
        }
    }

    fn process_queue(&mut self, mem: &mut Memory, queue_idx: usize) {
        let mut processed_any = false;
        
        loop {
            // STEP 1: Borrow queue (same logic as before)
            let (head_idx, input_buffer, output_descriptors) = {
                let queue = if let Some(q) = self.virtio.queues.get_mut(queue_idx) {
                    q
                } else {
                    return;
                };
                
                if !queue.ready { return; }
                let avail_idx = queue.avail_idx(mem);
                if queue.last_avail_idx == avail_idx { break; }
                
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
                    if (desc.flags & super::virtio::VRING_DESC_F_NEXT) == 0 { break; }
                    desc_idx = desc.next;
                }
                (head_idx, input, output)
            };
            
            // STEP 2: Process message
            let result = self.process_message(&input_buffer);
            
            match result {
                Some(response) => {
                    // STEP 3: Write response
                    {
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
                    // Suspended
                     let tag = if input_buffer.len() >= 7 {
                        u16::from_le_bytes([input_buffer[5], input_buffer[6]])
                    } else { 0xFFFF };
                    self.suspended_requests.push(SuspendedRequest {
                        queue_idx, head_idx, output_descriptors, tag, input_buffer: input_buffer.to_vec(),
                    });
                }
            }
        }
        
        if processed_any {
            self.virtio.raise_interrupt(true);
        }
    }
    
    pub fn notify(&mut self, _queue: u32) {}
    
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
            P9_TREAD => Some(self.handle_read(tag, payload)),
            P9_TWRITE => Some(self.handle_write(tag, payload)),
            P9_TMKDIR => Some(self.handle_mkdir(tag, payload)),
            P9_TMKNOD => Some(self.handle_mknod(tag, payload)), // Treat as create
            P9_TLCREATE => Some(self.handle_lcreate(tag, payload)),
            P9_TUNLINKAT => Some(self.handle_unlinkat(tag, payload)),
            P9_TRENAME => Some(self.handle_rename(tag, payload)),
            P9_TSTATFS => Some(self.error_response(tag, 12)), // ENOMEM not supported yet
            _ => Some(self.error_response(tag, EINVAL)),
        }
    }
    
    fn error_response(&self, tag: u16, errno: u32) -> Vec<u8> {
        let mut resp = Vec::new();
        resp.extend_from_slice(&0u32.to_le_bytes());
        resp.push(P9_RLERROR);
        resp.extend_from_slice(&tag.to_le_bytes());
        resp.extend_from_slice(&errno.to_le_bytes());
        let size = resp.len() as u32;
        resp[0..4].copy_from_slice(&size.to_le_bytes());
        resp
    }

    fn handle_version(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 6 { return self.error_response(tag, EINVAL); }
        let msize = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        self.msize = msize.min(8192);
        let version = b"9P2000.L";
        let mut resp = Vec::new();
        resp.extend_from_slice(&0u32.to_le_bytes());
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
        if payload.len() < 12 { return self.error_response(tag, EINVAL); }
        let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        
        match self.fs.attach() {
            Ok(qid) => {
                self.fids.insert(fid, Fid {
                    qid,
                    open: false,
                    open_flags: 0,
                    position: 0,
                });
                let mut resp = Vec::new();
                resp.extend_from_slice(&0u32.to_le_bytes());
                resp.push(P9_RATTACH);
                resp.extend_from_slice(&tag.to_le_bytes());
                resp.extend_from_slice(&qid.encode());
                let size = resp.len() as u32;
                resp[0..4].copy_from_slice(&size.to_le_bytes());
                resp
            },
            Err(e) => self.error_response(tag, e)
        }
    }
    
    fn handle_walk(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 10 { return self.error_response(tag, EINVAL); }
        let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let newfid = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
        let nwname = u16::from_le_bytes([payload[8], payload[9]]) as usize;
        
        let mut current_fid = match self.fids.get(&fid) {
            Some(f) => f.clone(),
            None => return self.error_response(tag, EBADF),
        };
        
        let mut qids = Vec::new();
        let mut offset = 10;
        
        for _ in 0..nwname {
            if offset + 2 > payload.len() { return self.error_response(tag, EINVAL); }
            let name_len = u16::from_le_bytes([payload[offset], payload[offset+1]]) as usize;
            offset += 2;
            if offset + name_len > payload.len() { return self.error_response(tag, EINVAL); }
            let name = String::from_utf8_lossy(&payload[offset..offset+name_len]).to_string();
            offset += name_len;
            
            match self.fs.walk(&current_fid.qid, &name) {
                Ok(qid) => {
                    qids.push(qid.encode());
                    current_fid.qid = qid;
                },
                Err(e) => {
                    if qids.is_empty() { return self.error_response(tag, e); }
                    break;
                }
            }
        }
        
        if qids.len() == nwname {
             self.fids.insert(newfid, current_fid);
        }
        
        let mut resp = Vec::new();
        resp.extend_from_slice(&0u32.to_le_bytes());
        resp.push(P9_RWALK);
        resp.extend_from_slice(&tag.to_le_bytes());
        resp.extend_from_slice(&(qids.len() as u16).to_le_bytes());
        for q in qids { resp.extend_from_slice(&q); }
        let size = resp.len() as u32;
        resp[0..4].copy_from_slice(&size.to_le_bytes());
        resp
    }

    fn handle_clunk(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 4 { return self.error_response(tag, EINVAL); }
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
        if payload.len() < 8 { return self.error_response(tag, EINVAL); }
        let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let f = match self.fids.get(&fid) {
            Some(f) => f,
            None => return self.error_response(tag, EBADF),
        };
        
        match self.fs.getattr(&f.qid) {
            Ok(attr) => {
                let mut resp = Vec::new();
                resp.extend_from_slice(&0u32.to_le_bytes());
                resp.push(P9_RGETATTR);
                resp.extend_from_slice(&tag.to_le_bytes());
                resp.extend_from_slice(&0x7fffu64.to_le_bytes()); // valid
                resp.extend_from_slice(&attr.qid.encode());
                resp.extend_from_slice(&attr.mode.to_le_bytes());
                resp.extend_from_slice(&attr.uid.to_le_bytes());
                resp.extend_from_slice(&attr.gid.to_le_bytes());
                resp.extend_from_slice(&attr.nlink.to_le_bytes());
                resp.extend_from_slice(&attr.rdev.to_le_bytes());
                resp.extend_from_slice(&attr.size.to_le_bytes());
                resp.extend_from_slice(&attr.blksize.to_le_bytes());
                resp.extend_from_slice(&attr.blocks.to_le_bytes());
                resp.extend_from_slice(&attr.atime.0.to_le_bytes());
                resp.extend_from_slice(&attr.atime.1.to_le_bytes());
                resp.extend_from_slice(&attr.mtime.0.to_le_bytes());
                resp.extend_from_slice(&attr.mtime.1.to_le_bytes());
                resp.extend_from_slice(&attr.ctime.0.to_le_bytes());
                resp.extend_from_slice(&attr.ctime.1.to_le_bytes());
                resp.extend_from_slice(&0u64.to_le_bytes()); // btime
                resp.extend_from_slice(&0u64.to_le_bytes());
                resp.extend_from_slice(&0u64.to_le_bytes()); // gen
                resp.extend_from_slice(&0u64.to_le_bytes()); // data_version
                let size = resp.len() as u32;
                resp[0..4].copy_from_slice(&size.to_le_bytes());
                resp
            },
            Err(e) => self.error_response(tag, e)
        }
    }

    fn handle_lopen(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 8 { return self.error_response(tag, EINVAL); }
        let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let flags = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
        
        if let Some(f) = self.fids.get_mut(&fid) {
            match self.fs.open(&f.qid, flags) {
                Ok(_) => {
                    f.open = true;
                    f.open_flags = flags;
                    f.position = 0;
                    
                    let mut resp = Vec::new();
                    resp.extend_from_slice(&0u32.to_le_bytes());
                    resp.push(P9_RLOPEN);
                    resp.extend_from_slice(&tag.to_le_bytes());
                    resp.extend_from_slice(&f.qid.encode());
                    resp.extend_from_slice(&4096u32.to_le_bytes()); // iounit
                    let size = resp.len() as u32;
                    resp[0..4].copy_from_slice(&size.to_le_bytes());
                    resp
                },
                Err(e) => self.error_response(tag, e)
            }
        } else {
            self.error_response(tag, EBADF)
        }
    }
    
    fn handle_read(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 12 { return self.error_response(tag, EINVAL); }
        let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let offset = u64::from_le_bytes([payload[4], payload[5], payload[6], payload[7], payload[8], payload[9], payload[10], payload[11]]);
        let count = u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
        
        let f = match self.fids.get(&fid) { Some(f) => f, None => return self.error_response(tag, EBADF) };
        
        match self.fs.read(&f.qid, offset, count) {
            Ok(data) => {
                let mut resp = Vec::new();
                resp.extend_from_slice(&0u32.to_le_bytes());
                resp.push(P9_RREAD);
                resp.extend_from_slice(&tag.to_le_bytes());
                resp.extend_from_slice(&(data.len() as u32).to_le_bytes());
                resp.extend_from_slice(&data);
                let size = resp.len() as u32;
                resp[0..4].copy_from_slice(&size.to_le_bytes());
                resp
            },
            Err(e) => self.error_response(tag, e)
        }
    }
    
    fn handle_write(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 16 { return self.error_response(tag, EINVAL); }
        let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let offset = u64::from_le_bytes([payload[4], payload[5], payload[6], payload[7], payload[8], payload[9], payload[10], payload[11]]);
        let count = u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
        if payload.len() < 16 + count as usize { return self.error_response(tag, EINVAL); }
        let data = &payload[16..16+count as usize];
        
        let f = match self.fids.get(&fid) { Some(f) => f, None => return self.error_response(tag, EBADF) };
        
        match self.fs.write(&f.qid, offset, data) {
             Ok(written) => {
                let mut resp = Vec::new();
                resp.extend_from_slice(&0u32.to_le_bytes());
                resp.push(P9_RWRITE);
                resp.extend_from_slice(&tag.to_le_bytes());
                resp.extend_from_slice(&written.to_le_bytes());
                let size = resp.len() as u32;
                resp[0..4].copy_from_slice(&size.to_le_bytes());
                resp
             },
             Err(e) => self.error_response(tag, e)
        }
    }
    
    fn handle_readdir(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 16 { return self.error_response(tag, EINVAL); }
        let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let offset = u64::from_le_bytes([payload[4], payload[5], payload[6], payload[7], payload[8], payload[9], payload[10], payload[11]]);
        let count = u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
        
        let f = match self.fids.get(&fid) { Some(f) => f, None => return self.error_response(tag, EBADF) };
        
        match self.fs.readdir(&f.qid, offset, count) {
            Ok(entries) => {
                let mut resp_data = Vec::new();
                for entry in entries {
                    let mut e_bytes = Vec::new();
                    e_bytes.extend_from_slice(&entry.qid.encode());
                    e_bytes.extend_from_slice(&entry.offset.to_le_bytes());
                    e_bytes.push(entry.type_);
                    let name_bytes = entry.name.as_bytes();
                    e_bytes.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
                    e_bytes.extend_from_slice(name_bytes);
                    
                    if resp_data.len() + e_bytes.len() > count as usize { break; }
                    resp_data.extend_from_slice(&e_bytes);
                }
                
                let mut resp = Vec::new();
                resp.extend_from_slice(&0u32.to_le_bytes());
                resp.push(P9_RREADDIR);
                resp.extend_from_slice(&tag.to_le_bytes());
                resp.extend_from_slice(&(resp_data.len() as u32).to_le_bytes());
                resp.extend_from_slice(&resp_data);
                let size = resp.len() as u32;
                resp[0..4].copy_from_slice(&size.to_le_bytes());
                resp
            },
            Err(e) => self.error_response(tag, e)
        }
    }

    fn handle_mkdir(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        if payload.len() < 10 { return self.error_response(tag, EINVAL); }
         let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
         // name ...
         // This parsing is tedious. 
         let name_len = u16::from_le_bytes([payload[4], payload[5]]) as usize;
         let name = String::from_utf8_lossy(&payload[6..6+name_len]).to_string();
         let offset = 6 + name_len;
         let mode = u32::from_le_bytes([payload[offset], payload[offset+1], payload[offset+2], payload[offset+3]]);
         
         let f = match self.fids.get(&fid) { Some(f) => f, None => return self.error_response(tag, EBADF) };
         
         match self.fs.mkdir(&f.qid, &name, mode) {
             Ok(qid) => {
                 let mut resp = Vec::new();
                 resp.extend_from_slice(&0u32.to_le_bytes());
                 resp.push(P9_RMKDIR);
                 resp.extend_from_slice(&tag.to_le_bytes());
                 resp.extend_from_slice(&qid.encode());
                 let size = resp.len() as u32;
                 resp[0..4].copy_from_slice(&size.to_le_bytes());
                 resp
             },
             Err(e) => self.error_response(tag, e)
        }
    }
    
    fn handle_mknod(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        // Treat mknod as create file for now if it is not special?
        self.error_response(tag, 1) // EPERM
    }
    
    fn handle_lcreate(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
         let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
         let name_len = u16::from_le_bytes([payload[4], payload[5]]) as usize;
         let name = String::from_utf8_lossy(&payload[6..6+name_len]).to_string();
         let offset = 6 + name_len;
         let flags = u32::from_le_bytes([payload[offset], payload[offset+1], payload[offset+2], payload[offset+3]]);
         let mode = u32::from_le_bytes([payload[offset+4], payload[offset+5], payload[offset+6], payload[offset+7]]);
         
         if let Some(f) = self.fids.get_mut(&fid) {
             match self.fs.create(&f.qid, &name, mode, flags) {
                 Ok(qid) => {
                     // LCREATE updates the fid to point to the new file and opens it
                     f.qid = qid;
                     f.open = true;
                     f.open_flags = flags;
                     f.position = 0;
                     
                     let mut resp = Vec::new();
                     resp.extend_from_slice(&0u32.to_le_bytes());
                     resp.push(P9_RLCREATE);
                     resp.extend_from_slice(&tag.to_le_bytes());
                     resp.extend_from_slice(&qid.encode());
                     resp.extend_from_slice(&4096u32.to_le_bytes()); // iounit
                     let size = resp.len() as u32;
                     resp[0..4].copy_from_slice(&size.to_le_bytes());
                     resp
                 },
                 Err(e) => self.error_response(tag, e)
             }
         } else {
             self.error_response(tag, EBADF)
         }
    }

    fn handle_unlinkat(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
        let _dirfid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let name_len = u16::from_le_bytes([payload[4], payload[5]]) as usize;
         let name = String::from_utf8_lossy(&payload[6..6+name_len]).to_string();
         
         // Standard unlinkat(dirfd, name)
         // Our remove() takes QID. We need to walk to it first?
         // Or does user usually walk then unlink?
         // Actually Tremove operates on a FID. Tunlinkat is newer.
         // Tunlinkat: dirfd, name, flags.
         
         // We'll trust the user has the right permissions in the parent.
         // We need to find the child QID.
         if let Some(f) = self.fids.get(&_dirfid) {
              match self.fs.walk(&f.qid, &name) {
                  Ok(qid) => {
                      match self.fs.remove(&qid) {
                          Ok(_) => {
                              let mut resp = Vec::new();
                                resp.extend_from_slice(&0u32.to_le_bytes());
                                resp.push(P9_RUNLINKAT);
                                resp.extend_from_slice(&tag.to_le_bytes());
                                let size = resp.len() as u32;
                                resp[0..4].copy_from_slice(&size.to_le_bytes());
                                resp
                          },
                          Err(e) => self.error_response(tag, e)
                      }
                  },
                  Err(e) => self.error_response(tag, e)
              }
         } else {
             self.error_response(tag, EBADF)
         }
    }

    fn handle_rename(&mut self, tag: u16, payload: &[u8]) -> Vec<u8> {
         // Trename (uuid[4] dirfid[4] name[s])
         // changes name of file identified by fid to name in directory dirfid
         let fid = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
         let dirfid = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
         let name_len = u16::from_le_bytes([payload[8], payload[9]]) as usize;
         let name = String::from_utf8_lossy(&payload[10..10+name_len]).to_string();
         
         let file_qid = if let Some(f) = self.fids.get(&fid) { f.qid } else { return self.error_response(tag, EBADF); };
         let dir_qid = if let Some(f) = self.fids.get(&dirfid) { f.qid } else { return self.error_response(tag, EBADF); };
         
         match self.fs.rename(&file_qid, &dir_qid, &name) {
             Ok(_) => {
                 let mut resp = Vec::new();
                resp.extend_from_slice(&0u32.to_le_bytes());
                resp.push(P9_RRENAME);
                resp.extend_from_slice(&tag.to_le_bytes());
                let size = resp.len() as u32;
                resp[0..4].copy_from_slice(&size.to_le_bytes());
                resp
             },
             Err(e) => self.error_response(tag, e)
         }
    }
}
