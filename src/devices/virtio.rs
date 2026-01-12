//! VirtIO MMIO device implementation
//!
//! Implements the VirtIO MMIO transport layer for devices like 9p filesystem.
//! Based on VirtIO 1.0 specification.

use std::collections::VecDeque;

// VirtIO MMIO register offsets
const VIRTIO_MMIO_MAGIC_VALUE: u32 = 0x000;
const VIRTIO_MMIO_VERSION: u32 = 0x004;
const VIRTIO_MMIO_DEVICE_ID: u32 = 0x008;
const VIRTIO_MMIO_VENDOR_ID: u32 = 0x00c;
const VIRTIO_MMIO_DEVICE_FEATURES: u32 = 0x010;
const VIRTIO_MMIO_DEVICE_FEATURES_SEL: u32 = 0x014;
const VIRTIO_MMIO_DRIVER_FEATURES: u32 = 0x020;
const VIRTIO_MMIO_DRIVER_FEATURES_SEL: u32 = 0x024;
const VIRTIO_MMIO_QUEUE_SEL: u32 = 0x030;
const VIRTIO_MMIO_QUEUE_NUM_MAX: u32 = 0x034;
const VIRTIO_MMIO_QUEUE_NUM: u32 = 0x038;
const VIRTIO_MMIO_QUEUE_READY: u32 = 0x044;
const VIRTIO_MMIO_QUEUE_NOTIFY: u32 = 0x050;
const VIRTIO_MMIO_INTERRUPT_STATUS: u32 = 0x060;
const VIRTIO_MMIO_INTERRUPT_ACK: u32 = 0x064;
const VIRTIO_MMIO_STATUS: u32 = 0x070;
const VIRTIO_MMIO_QUEUE_DESC_LOW: u32 = 0x080;
const VIRTIO_MMIO_QUEUE_DESC_HIGH: u32 = 0x084;
const VIRTIO_MMIO_QUEUE_AVAIL_LOW: u32 = 0x090;
const VIRTIO_MMIO_QUEUE_AVAIL_HIGH: u32 = 0x094;
const VIRTIO_MMIO_QUEUE_USED_LOW: u32 = 0x0a0;
const VIRTIO_MMIO_QUEUE_USED_HIGH: u32 = 0x0a4;
const VIRTIO_MMIO_CONFIG_GENERATION: u32 = 0x0fc;
const VIRTIO_MMIO_CONFIG: u32 = 0x100;

// VirtIO magic value
const VIRTIO_MAGIC: u32 = 0x74726976; // "virt"

// VirtIO device IDs
const VIRTIO_DEV_NET: u32 = 1;
const VIRTIO_DEV_BLK: u32 = 2;
const VIRTIO_DEV_9P: u32 = 9;

// VirtIO status bits
const VIRTIO_STATUS_ACKNOWLEDGE: u32 = 1;
const VIRTIO_STATUS_DRIVER: u32 = 2;
const VIRTIO_STATUS_DRIVER_OK: u32 = 4;
const VIRTIO_STATUS_FEATURES_OK: u32 = 8;
const VIRTIO_STATUS_DEVICE_NEEDS_RESET: u32 = 64;
const VIRTIO_STATUS_FAILED: u32 = 128;

// Virtqueue descriptor flags
const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;
const VRING_DESC_F_INDIRECT: u16 = 4;

/// A single virtqueue
pub struct Virtqueue {
    /// Maximum queue size
    pub num_max: u32,
    /// Current queue size
    pub num: u32,
    /// Queue is ready
    pub ready: bool,
    /// Descriptor table address
    pub desc_addr: u64,
    /// Available ring address
    pub avail_addr: u64,
    /// Used ring address
    pub used_addr: u64,
    /// Last available index we processed
    pub last_avail_idx: u16,
}

impl Virtqueue {
    pub fn new(num_max: u32) -> Self {
        Virtqueue {
            num_max,
            num: 0,
            ready: false,
            desc_addr: 0,
            avail_addr: 0,
            used_addr: 0,
            last_avail_idx: 0,
        }
    }
    
    pub fn reset(&mut self) {
        self.num = 0;
        self.ready = false;
        self.desc_addr = 0;
        self.avail_addr = 0;
        self.used_addr = 0;
        self.last_avail_idx = 0;
    }
}

/// VirtIO MMIO device base
pub struct VirtioMmio {
    /// Device type ID
    pub device_id: u32,
    /// Vendor ID
    pub vendor_id: u32,
    /// Device features
    pub device_features: u64,
    /// Driver-selected features
    pub driver_features: u64,
    /// Device feature selection (0 or 1 for low/high 32 bits)
    pub device_features_sel: u32,
    /// Driver feature selection
    pub driver_features_sel: u32,
    /// Current queue selection
    pub queue_sel: u32,
    /// Queues
    pub queues: Vec<Virtqueue>,
    /// Interrupt status
    pub interrupt_status: u32,
    /// Device status
    pub status: u32,
    /// Configuration data
    pub config: Vec<u8>,
    /// Configuration generation counter
    pub config_generation: u32,
    /// Interrupt pending
    pub interrupt_pending: bool,
}

impl VirtioMmio {
    pub fn new(device_id: u32, num_queues: usize, config: Vec<u8>) -> Self {
        let mut queues = Vec::with_capacity(num_queues);
        for _ in 0..num_queues {
            queues.push(Virtqueue::new(256));
        }
        
        VirtioMmio {
            device_id,
            vendor_id: 0x554D4551, // "QEMU"
            device_features: 0,
            driver_features: 0,
            device_features_sel: 0,
            driver_features_sel: 0,
            queue_sel: 0,
            queues,
            interrupt_status: 0,
            status: 0,
            config,
            config_generation: 0,
            interrupt_pending: false,
        }
    }
    
    pub fn read32(&self, offset: u32) -> u32 {
        match offset {
            VIRTIO_MMIO_MAGIC_VALUE => VIRTIO_MAGIC,
            VIRTIO_MMIO_VERSION => 2, // VirtIO 1.0+
            VIRTIO_MMIO_DEVICE_ID => self.device_id,
            VIRTIO_MMIO_VENDOR_ID => self.vendor_id,
            VIRTIO_MMIO_DEVICE_FEATURES => {
                let features = if self.device_features_sel == 0 {
                    self.device_features as u32
                } else {
                    (self.device_features >> 32) as u32
                };
                features
            }
            VIRTIO_MMIO_QUEUE_NUM_MAX => {
                if let Some(q) = self.queues.get(self.queue_sel as usize) {
                    q.num_max
                } else {
                    0
                }
            }
            VIRTIO_MMIO_QUEUE_READY => {
                if let Some(q) = self.queues.get(self.queue_sel as usize) {
                    if q.ready { 1 } else { 0 }
                } else {
                    0
                }
            }
            VIRTIO_MMIO_INTERRUPT_STATUS => self.interrupt_status,
            VIRTIO_MMIO_STATUS => self.status,
            VIRTIO_MMIO_CONFIG_GENERATION => self.config_generation,
            _ if offset >= VIRTIO_MMIO_CONFIG => {
                let config_offset = (offset - VIRTIO_MMIO_CONFIG) as usize;
                if config_offset + 4 <= self.config.len() {
                    u32::from_le_bytes([
                        self.config[config_offset],
                        self.config[config_offset + 1],
                        self.config[config_offset + 2],
                        self.config[config_offset + 3],
                    ])
                } else {
                    0
                }
            }
            _ => 0,
        }
    }
    
    pub fn write32(&mut self, offset: u32, value: u32) {
        match offset {
            VIRTIO_MMIO_DEVICE_FEATURES_SEL => {
                self.device_features_sel = value;
            }
            VIRTIO_MMIO_DRIVER_FEATURES => {
                if self.driver_features_sel == 0 {
                    self.driver_features = (self.driver_features & 0xFFFFFFFF00000000) | (value as u64);
                } else {
                    self.driver_features = (self.driver_features & 0xFFFFFFFF) | ((value as u64) << 32);
                }
            }
            VIRTIO_MMIO_DRIVER_FEATURES_SEL => {
                self.driver_features_sel = value;
            }
            VIRTIO_MMIO_QUEUE_SEL => {
                self.queue_sel = value;
            }
            VIRTIO_MMIO_QUEUE_NUM => {
                if let Some(q) = self.queues.get_mut(self.queue_sel as usize) {
                    q.num = value;
                }
            }
            VIRTIO_MMIO_QUEUE_READY => {
                if let Some(q) = self.queues.get_mut(self.queue_sel as usize) {
                    q.ready = value != 0;
                }
            }
            VIRTIO_MMIO_QUEUE_DESC_LOW => {
                if let Some(q) = self.queues.get_mut(self.queue_sel as usize) {
                    q.desc_addr = (q.desc_addr & 0xFFFFFFFF00000000) | (value as u64);
                }
            }
            VIRTIO_MMIO_QUEUE_DESC_HIGH => {
                if let Some(q) = self.queues.get_mut(self.queue_sel as usize) {
                    q.desc_addr = (q.desc_addr & 0xFFFFFFFF) | ((value as u64) << 32);
                }
            }
            VIRTIO_MMIO_QUEUE_AVAIL_LOW => {
                if let Some(q) = self.queues.get_mut(self.queue_sel as usize) {
                    q.avail_addr = (q.avail_addr & 0xFFFFFFFF00000000) | (value as u64);
                }
            }
            VIRTIO_MMIO_QUEUE_AVAIL_HIGH => {
                if let Some(q) = self.queues.get_mut(self.queue_sel as usize) {
                    q.avail_addr = (q.avail_addr & 0xFFFFFFFF) | ((value as u64) << 32);
                }
            }
            VIRTIO_MMIO_QUEUE_USED_LOW => {
                if let Some(q) = self.queues.get_mut(self.queue_sel as usize) {
                    q.used_addr = (q.used_addr & 0xFFFFFFFF00000000) | (value as u64);
                }
            }
            VIRTIO_MMIO_QUEUE_USED_HIGH => {
                if let Some(q) = self.queues.get_mut(self.queue_sel as usize) {
                    q.used_addr = (q.used_addr & 0xFFFFFFFF) | ((value as u64) << 32);
                }
            }
            VIRTIO_MMIO_INTERRUPT_ACK => {
                self.interrupt_status &= !value;
                if self.interrupt_status == 0 {
                    self.interrupt_pending = false;
                }
            }
            VIRTIO_MMIO_STATUS => {
                if value == 0 {
                    // Reset device
                    self.reset();
                } else {
                    self.status = value;
                }
            }
            _ => {}
        }
    }
    
    pub fn read8(&self, offset: u32) -> u8 {
        if offset >= VIRTIO_MMIO_CONFIG {
            let config_offset = (offset - VIRTIO_MMIO_CONFIG) as usize;
            if config_offset < self.config.len() {
                return self.config[config_offset];
            }
        }
        // Fallback to 32-bit read
        let aligned = offset & !3;
        let shift = (offset & 3) * 8;
        ((self.read32(aligned) >> shift) & 0xFF) as u8
    }
    
    pub fn write8(&mut self, offset: u32, value: u8) {
        if offset >= VIRTIO_MMIO_CONFIG {
            let config_offset = (offset - VIRTIO_MMIO_CONFIG) as usize;
            if config_offset < self.config.len() {
                self.config[config_offset] = value;
                self.config_generation = self.config_generation.wrapping_add(1);
            }
        }
        // Other registers don't support byte writes
    }
    
    pub fn reset(&mut self) {
        self.driver_features = 0;
        self.device_features_sel = 0;
        self.driver_features_sel = 0;
        self.queue_sel = 0;
        self.interrupt_status = 0;
        self.status = 0;
        for q in &mut self.queues {
            q.reset();
        }
        self.interrupt_pending = false;
    }
    
    /// Raise an interrupt
    pub fn raise_interrupt(&mut self, ring_update: bool) {
        if ring_update {
            self.interrupt_status |= 1; // Used buffer notification
        } else {
            self.interrupt_status |= 2; // Configuration change
        }
        self.interrupt_pending = true;
    }
}
