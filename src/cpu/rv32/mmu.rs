//! Sv32 MMU implementation
//!
//! Handles virtual address translation for 32-bit RISC-V.
//! Supports 4KB pages and 4MB megapages.

use crate::cpu::PrivilegeLevel;
use crate::memory::Bus;

/// Access type for translation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    Instruction,
    Load,
    Store,
}

const TLB_SIZE: usize = 16;

#[derive(Clone, Copy)]
struct TlbEntry {
    tag: u32,
    ppn: u32,
    perm: u8,
    valid: bool,
    generation: u32,
    page_shift: u8,
}

impl TlbEntry {
    const fn empty() -> Self {
        TlbEntry {
            tag: 0,
            ppn: 0,
            perm: 0,
            valid: false,
            generation: 0,
            page_shift: 12,
        }
    }
}

/// MMU State
pub struct Mmu {
    tlb: [TlbEntry; TLB_SIZE],
    tlb_generation: u32,
    tlb_hits: u64,
    tlb_misses: u64,
}

impl Mmu {
    pub fn new() -> Self {
        Mmu {
            tlb: [TlbEntry::empty(); TLB_SIZE],
            tlb_generation: 1,
            tlb_hits: 0,
            tlb_misses: 0,
        }
    }

    pub fn reset(&mut self) {
        self.tlb = [TlbEntry::empty(); TLB_SIZE];
        self.tlb_generation = 1;
        self.tlb_hits = 0;
        self.tlb_misses = 0;
    }

    pub fn invalidate(&mut self) {
        self.tlb_generation = self.tlb_generation.wrapping_add(1);
    }

    pub fn tlb_stats(&self) -> (u64, u64) {
        (self.tlb_hits, self.tlb_misses)
    }

    /// Translate a virtual address to a physical address
    /// 
    /// Returns:
    /// - Ok(paddr) if translation succeeds
    /// - Err(trap_cause) if translation fails (PageFault)
    pub fn translate(&mut self, vaddr: u32, access_type: AccessType, priv_level: PrivilegeLevel, bus: &mut impl Bus, satp: u32, mstatus: u32) -> Result<u32, u32> {
        // 1. Check if paging is enabled
        // Mode is satp[31]. 0 = Bare, 1 = Sv32
        let mode = (satp >> 31) & 1;
        
        // If in Machine mode, paging is disabled
        // STRICTLY: If MPRV=1, load/store might use translation (handled by caller passing effective priv).
        if (priv_level == PrivilegeLevel::Machine) || (mode == 0) {
            return Ok(vaddr);
        }

        if let Some(paddr) = self.tlb_lookup(vaddr, access_type, priv_level, mstatus) {
            return Ok(paddr);
        }

        // 2. Sv32 Translation (slow path)
        let ppn = satp & 0x3FFFFF;
        let root_page_table = ppn << 12;

        let vpn1 = (vaddr >> 22) & 0x3FF;
        let vpn0 = (vaddr >> 12) & 0x3FF;
        let page_offset = vaddr & 0xFFF;

        // Level 1 Walk
        let pte1_addr = root_page_table + (vpn1 * 4);
        let pte1 = bus.read32(pte1_addr);

        // Check valid bit (V=1)
        if (pte1 & 1) == 0 {
            return Err(self.page_fault_cause(access_type));
        }

        // Check if leaf PTE (R=1 or X=1)
        let r = (pte1 >> 1) & 1;
        let w = (pte1 >> 2) & 1;
        let x = (pte1 >> 3) & 1;
        let u = (pte1 >> 4) & 1;

        if (r == 1) || (x == 1) {
            // Megapage (4MB)
            // Permission check
            self.check_permissions(pte1, access_type, priv_level, mstatus)?;
            
            // Check misalignment: ppn0 must be 0 for megapage
            if ((pte1 >> 10) & 0x3FF) != 0 {
                return Err(self.page_fault_cause(access_type));
            }

            let phys_ppn1 = (pte1 >> 20) & 0xFFF;
            let paddr = (phys_ppn1 << 22) | (vpn0 << 12) | page_offset;
            
            // Update A/D bits
            if self.update_ad_bits(bus, pte1_addr, pte1, access_type)? {
                // If we wrote back, need to consider TLB flush if we had one
            }

            let perm = ((r as u8) << 0) | ((w as u8) << 1) | ((x as u8) << 2) | ((u as u8) << 3);
            let tag = vaddr >> 22;
            self.fill_tlb(tag, phys_ppn1, perm, 22);
            
            return Ok(paddr);
        }

        // Pointer to next level
        let next_table_ppn = (pte1 >> 10) & 0x3FFFFF;
        let pte0_addr = (next_table_ppn << 12) + (vpn0 * 4);
        let pte0 = bus.read32(pte0_addr);

        if (pte0 & 1) == 0 {
            return Err(self.page_fault_cause(access_type));
        }
        
        // Leaf PTE check (Level 0 MUST be leaf)
        let r0 = (pte0 >> 1) & 1;
        let w0 = (pte0 >> 2) & 1;
        let x0 = (pte0 >> 3) & 1;
        let u0 = (pte0 >> 4) & 1;
        if (r0 == 0) && (x0 == 0) {
             return Err(self.page_fault_cause(access_type)); 
        }

        // Permission check
        self.check_permissions(pte0, access_type, priv_level, mstatus)?;

        // 4KB Page
        let ppn = (pte0 >> 10) & 0x3FFFFF;
        let paddr = (ppn << 12) | page_offset;
        
        // Update A/D bits
        self.update_ad_bits(bus, pte0_addr, pte0, access_type)?;

        let perm = ((r0 as u8) << 0) | ((w0 as u8) << 1) | ((x0 as u8) << 2) | ((u0 as u8) << 3);
        let tag = vaddr >> 12;
        self.fill_tlb(tag, ppn, perm, 12);

        Ok(paddr)
    }

    fn tlb_lookup(&mut self, vaddr: u32, access_type: AccessType, priv_level: PrivilegeLevel, mstatus: u32) -> Option<u32> {
        // Check 4KB entry first
        let tag_4k = vaddr >> 12;
        let idx_4k = (tag_4k as usize) & (TLB_SIZE - 1);
        if let Some(paddr) = self.tlb_match(idx_4k, tag_4k, 12, vaddr, access_type, priv_level, mstatus) {
            self.tlb_hits += 1;
            return Some(paddr);
        }

        // Check 4MB entry
        let tag_4m = vaddr >> 22;
        let idx_4m = (tag_4m as usize) & (TLB_SIZE - 1);
        if let Some(paddr) = self.tlb_match(idx_4m, tag_4m, 22, vaddr, access_type, priv_level, mstatus) {
            self.tlb_hits += 1;
            return Some(paddr);
        }

        self.tlb_misses += 1;
        None
    }

    fn tlb_match(&self, idx: usize, tag: u32, page_shift: u8, vaddr: u32, access_type: AccessType, priv_level: PrivilegeLevel, mstatus: u32) -> Option<u32> {
        let entry = self.tlb[idx];
        if !entry.valid || entry.generation != self.tlb_generation {
            return None;
        }
        if entry.tag != tag || entry.page_shift != page_shift {
            return None;
        }
        if !self.check_perm_fast(entry.perm, access_type, priv_level, mstatus) {
            return None;
        }

        let paddr = match entry.page_shift {
            12 => (entry.ppn << 12) | (vaddr & 0x0FFF),
            22 => (entry.ppn << 22) | (vaddr & 0x003F_FFFF),
            _ => return None,
        };
        Some(paddr)
    }

    fn fill_tlb(&mut self, tag: u32, ppn: u32, perm: u8, page_shift: u8) {
        let idx = (tag as usize) & (TLB_SIZE - 1);
        self.tlb[idx] = TlbEntry {
            tag,
            ppn,
            perm,
            valid: true,
            generation: self.tlb_generation,
            page_shift,
        };
    }
    
    fn check_permissions(&self, pte: u32, access_type: AccessType, priv_level: PrivilegeLevel, mstatus: u32) -> Result<(), u32> {
        let r = (pte >> 1) & 1;
        let w = (pte >> 2) & 1;
        let x = (pte >> 3) & 1;
        let u = (pte >> 4) & 1; // User bit
        
        // If U=1, S-mode cannot access usually
        if (priv_level == PrivilegeLevel::Supervisor) && (u == 1) {
             // Check MSTATUS.SUM (bit 18)
             let sum = (mstatus >> 18) & 1;
             if sum == 0 {
                 return Err(self.page_fault_cause(access_type));
             }
        }
        
        // If U=0, U-mode cannot access
        if (priv_level == PrivilegeLevel::User) && (u == 0) {
            return Err(self.page_fault_cause(access_type));
        }
        
        // MXR (Make Executable Readable) - bit 19
        let mxr = (mstatus >> 19) & 1;
        
        match access_type {
             AccessType::Instruction => {
                 if x == 0 { return Err(12); } // Fetch page fault
             },
             AccessType::Load => {
                 // Readable results in success.
                 // If not readable, check if Executable and MXR=1
                 if r == 1 {
                     return Ok(());
                 }
                 if x == 1 && mxr == 1 {
                     return Ok(());
                 }
                 return Err(13); // Load page fault
             },
             AccessType::Store => {
                 if w == 0 { return Err(15); } // Store page fault
             }
        }
        Ok(())
    }

    fn check_perm_fast(&self, perm: u8, access_type: AccessType, priv_level: PrivilegeLevel, mstatus: u32) -> bool {
        let r = (perm & 0x01) != 0;
        let w = (perm & 0x02) != 0;
        let x = (perm & 0x04) != 0;
        let u = (perm & 0x08) != 0;

        if priv_level == PrivilegeLevel::Supervisor && u {
            let sum = (mstatus >> 18) & 1;
            if sum == 0 {
                return false;
            }
        }

        if priv_level == PrivilegeLevel::User && !u {
            return false;
        }

        let mxr = (mstatus >> 19) & 1;

        match access_type {
            AccessType::Instruction => x,
            AccessType::Load => r || (x && mxr == 1),
            AccessType::Store => w,
        }
    }
    
    fn update_ad_bits(&self, bus: &mut impl Bus, pte_addr: u32, pte: u32, access_type: AccessType) -> Result<bool, u32> {
        let mut new_pte = pte;
        let a = (pte >> 6) & 1;
        let d = (pte >> 7) & 1;
        
        // Set Accessed bit
        if a == 0 {
            new_pte |= 1 << 6;
        }
        
        // Set Dirty bit on stores
        if access_type == AccessType::Store && d == 0 {
            new_pte |= 1 << 7;
        }
        
        if new_pte != pte {
            bus.write32(pte_addr, new_pte);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn page_fault_cause(&self, access_type: AccessType) -> u32 {
        // CAUSE_FETCH_PAGE_FAULT = 12
        // CAUSE_LOAD_PAGE_FAULT = 13
        // CAUSE_STORE_PAGE_FAULT = 15
        match access_type {
            AccessType::Instruction => 12,
            AccessType::Load => 13,
            AccessType::Store => 15,
        }
    }
}

impl Default for Mmu {
    fn default() -> Self {
        Mmu::new()
    }
}
