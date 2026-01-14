//! Instruction Cache
//!
//! Caches decoded instructions per physical page to avoid
//! repeated decoding of hot code paths.

use std::collections::HashMap;

/// Number of instruction slots per page (4KB / 4 bytes)
const INSTS_PER_PAGE: usize = 1024;

/// Cached decoded instruction - minimal representation
#[derive(Clone, Copy, Default)]
pub struct CachedInst {
    /// Raw instruction (for validation and immediate extraction)
    pub raw: u32,
    /// Opcode (7 bits)
    pub opcode: u8,
    /// Destination register (5 bits)
    pub rd: u8,
    /// Source register 1 (5 bits)
    pub rs1: u8,
    /// Source register 2 (5 bits)
    pub rs2: u8,
    /// rs3 for R4-type (5 bits)
    pub rs3: u8,
    /// funct3 (3 bits)
    pub funct3: u8,
    /// funct7 (7 bits)
    pub funct7: u8,
    /// Valid flag
    pub valid: bool,
}

impl CachedInst {
    #[inline(always)]
    pub fn decode(raw: u32) -> Self {
        CachedInst {
            raw,
            opcode: (raw & 0x7F) as u8,
            rd: ((raw >> 7) & 0x1F) as u8,
            rs1: ((raw >> 15) & 0x1F) as u8,
            rs2: ((raw >> 20) & 0x1F) as u8,
            rs3: ((raw >> 27) & 0x1F) as u8,
            funct3: ((raw >> 12) & 0x7) as u8,
            funct7: ((raw >> 25) & 0x7F) as u8,
            valid: true,
        }
    }
}

/// Cached page of instructions
struct CachedPage {
    instructions: Box<[CachedInst; INSTS_PER_PAGE]>,
    generation: u32,
}

impl CachedPage {
    fn new(generation: u32) -> Self {
        CachedPage {
            instructions: Box::new([CachedInst::default(); INSTS_PER_PAGE]),
            generation,
        }
    }
}

/// Instruction cache
pub struct ICache {
    /// Map from physical page number (paddr >> 12) to cached page
    pages: HashMap<u32, CachedPage>,
    /// Generation counter for invalidation
    generation: u32,
    /// Stats
    pub hits: u64,
    pub misses: u64,
}

impl Default for ICache {
    fn default() -> Self {
        ICache::new()
    }
}

impl ICache {
    pub fn new() -> Self {
        ICache {
            pages: HashMap::with_capacity(64),
            generation: 1,
            hits: 0,
            misses: 0,
        }
    }

    /// Lookup or decode an instruction at physical address
    #[inline(always)]
    pub fn get_or_decode(&mut self, paddr: u32, raw_inst: u32) -> CachedInst {
        let page_num = paddr >> 12;
        let offset = ((paddr >> 2) & 0x3FF) as usize;

        if let Some(page) = self.pages.get(&page_num) {
            if page.generation == self.generation {
                let cached = &page.instructions[offset];
                if cached.valid && cached.raw == raw_inst {
                    self.hits += 1;
                    return *cached;
                }
            }
        }

        // Cache miss - decode and store
        self.misses += 1;
        let decoded = CachedInst::decode(raw_inst);
        
        // Insert into cache
        let page = self.pages.entry(page_num).or_insert_with(|| CachedPage::new(self.generation));
        if page.generation != self.generation {
            // Stale page, reset it
            page.generation = self.generation;
            page.instructions = Box::new([CachedInst::default(); INSTS_PER_PAGE]);
        }
        page.instructions[offset] = decoded;
        
        decoded
    }

    /// Invalidate cache entry for a physical address (for self-modifying code)
    #[inline(always)]
    pub fn invalidate_addr(&mut self, paddr: u32) {
        let page_num = paddr >> 12;
        let offset = ((paddr >> 2) & 0x3FF) as usize;
        
        if let Some(page) = self.pages.get_mut(&page_num) {
            if page.generation == self.generation {
                page.instructions[offset].valid = false;
            }
        }
    }

    /// Invalidate entire cache (e.g., on FENCE.I)
    pub fn invalidate_all(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Reset cache
    pub fn reset(&mut self) {
        self.pages.clear();
        self.generation = 1;
        self.hits = 0;
        self.misses = 0;
    }

    /// Get hit rate
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}
