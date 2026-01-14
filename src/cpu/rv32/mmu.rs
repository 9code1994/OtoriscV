//! Sv32 MMU implementation
//!
//! Handles virtual address translation for 32-bit RISC-V.
//! Supports 4KB pages and 4MB megapages.
//! Uses jor1k-style XOR TLB for ultra-fast translation.

use crate::cpu::PrivilegeLevel;
use crate::memory::Bus;

/// Access type for translation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    Instruction = 0,
    Load = 1,
    Store = 2,
}

/// jor1k-style XOR TLB entry
/// 
/// The key insight: store (paddr XOR vaddr) masked to page boundary.
/// On hit, a single XOR gives the physical address.
#[derive(Clone, Copy)]
struct XorTlbEntry {
    /// vaddr check value (vaddr & page_mask)
    check: u32,
    /// XOR lookup: ((paddr ^ vaddr) & page_mask)
    /// To get paddr: lookup ^ vaddr
    lookup: u32,
    /// Page mask (inverted offset mask): 0xFFFFF000 for 4KB, 0xFFC00000 for 4MB
    page_mask: u32,
}

impl XorTlbEntry {
    const fn empty() -> Self {
        XorTlbEntry {
            check: 0xFFFF_FFFF,  // Invalid - won't match any vaddr
            lookup: 0,
            page_mask: 0xFFFF_F000,
        }
    }
}

/// MMU State with jor1k-style XOR TLB
pub struct Mmu {
    /// XOR TLB entries: [Instruction, Load, Store]
    tlb: [XorTlbEntry; 3],
    
    /// Generation for lazy invalidation
    generation: u32,
    tlb_gen: [u32; 3],
    
    /// Stats
    tlb_hits: u64,
    tlb_misses: u64,
}

impl Default for Mmu {
    fn default() -> Self {
        Mmu::new()
    }
}

impl Mmu {
    pub fn new() -> Self {
        Mmu {
            tlb: [XorTlbEntry::empty(); 3],
            generation: 1,
            tlb_gen: [0; 3],
            tlb_hits: 0,
            tlb_misses: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Mmu::new();
    }

    #[inline(always)]
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn tlb_stats(&self) -> (u64, u64) {
        (self.tlb_hits, self.tlb_misses)
    }

    /// Ultra-fast translation with jor1k-style XOR TLB
    #[inline(always)]
    pub fn translate(&mut self, vaddr: u32, access_type: AccessType, priv_level: PrivilegeLevel, bus: &mut impl Bus, satp: u32, mstatus: u32) -> Result<u32, u32> {
        // Fast path: Machine mode or paging disabled
        let mode = satp >> 31;
        if priv_level == PrivilegeLevel::Machine || mode == 0 {
            return Ok(vaddr);
        }

        // XOR TLB lookup - single comparison, single XOR on hit
        let idx = access_type as usize;
        let entry = &self.tlb[idx];
        
        // Check: (entry.check XOR vaddr) masked should be 0 for hit
        if self.tlb_gen[idx] == self.generation && 
           (entry.check ^ vaddr) & entry.page_mask == 0 {
            // HIT: single XOR gives physical address
            self.tlb_hits += 1;
            return Ok(entry.lookup ^ vaddr);
        }

        // TLB miss - do full page walk
        self.tlb_misses += 1;
        self.translate_slow(vaddr, access_type, priv_level, bus, satp, mstatus)
    }

    /// Slow path: full page table walk
    #[cold]
    fn translate_slow(&mut self, vaddr: u32, access_type: AccessType, priv_level: PrivilegeLevel, bus: &mut impl Bus, satp: u32, mstatus: u32) -> Result<u32, u32> {
        let ppn = satp & 0x3FFFFF;
        let root_page_table = ppn << 12;

        let vpn1 = (vaddr >> 22) & 0x3FF;
        let vpn0 = (vaddr >> 12) & 0x3FF;
        let page_offset = vaddr & 0xFFF;

        // Level 1 Walk
        let pte1_addr = root_page_table + (vpn1 * 4);
        let pte1 = bus.read32(pte1_addr);

        if (pte1 & 1) == 0 {
            return Err(self.page_fault_cause(access_type));
        }

        let r = (pte1 >> 1) & 1;
        let x = (pte1 >> 3) & 1;

        if (r == 1) || (x == 1) {
            // Megapage (4MB)
            self.check_permissions(pte1, access_type, priv_level, mstatus)?;
            
            if ((pte1 >> 10) & 0x3FF) != 0 {
                return Err(self.page_fault_cause(access_type));
            }

            let phys_ppn1 = (pte1 >> 20) & 0xFFF;
            let pa_base = phys_ppn1 << 22;
            let paddr = pa_base | (vpn0 << 12) | page_offset;
            
            self.update_ad_bits(bus, pte1_addr, pte1, access_type)?;
            
            // Fill XOR TLB with megapage entry (4MB)
            let page_mask = 0xFFC0_0000u32;  // 4MB page mask
            self.fill_xor_tlb(access_type, vaddr, paddr, page_mask);
            
            return Ok(paddr);
        }

        // Level 0 walk
        let next_table_ppn = (pte1 >> 10) & 0x3FFFFF;
        let pte0_addr = (next_table_ppn << 12) + (vpn0 * 4);
        let pte0 = bus.read32(pte0_addr);

        if (pte0 & 1) == 0 {
            return Err(self.page_fault_cause(access_type));
        }
        
        let r0 = (pte0 >> 1) & 1;
        let x0 = (pte0 >> 3) & 1;
        if (r0 == 0) && (x0 == 0) {
            return Err(self.page_fault_cause(access_type)); 
        }

        self.check_permissions(pte0, access_type, priv_level, mstatus)?;

        let final_ppn = (pte0 >> 10) & 0x3FFFFF;
        let pa_base = final_ppn << 12;
        let paddr = pa_base | page_offset;
        
        self.update_ad_bits(bus, pte0_addr, pte0, access_type)?;

        // Fill XOR TLB with 4KB entry
        let page_mask = 0xFFFF_F000u32;  // 4KB page mask
        self.fill_xor_tlb(access_type, vaddr, paddr, page_mask);

        Ok(paddr)
    }

    /// Fill XOR TLB entry (jor1k style)
    /// 
    /// Stores: check = vaddr, lookup = paddr ^ vaddr (masked)
    /// On hit: paddr = lookup ^ vaddr
    #[inline(always)]
    fn fill_xor_tlb(&mut self, access_type: AccessType, vaddr: u32, paddr: u32, page_mask: u32) {
        let idx = access_type as usize;
        self.tlb[idx] = XorTlbEntry {
            check: vaddr,
            lookup: (paddr ^ vaddr) & page_mask,
            page_mask,
        };
        self.tlb_gen[idx] = self.generation;
    }
    
    fn check_permissions(&self, pte: u32, access_type: AccessType, priv_level: PrivilegeLevel, mstatus: u32) -> Result<(), u32> {
        let r = (pte >> 1) & 1;
        let w = (pte >> 2) & 1;
        let x = (pte >> 3) & 1;
        let u = (pte >> 4) & 1;
        
        if (priv_level == PrivilegeLevel::Supervisor) && (u == 1) {
            let sum = (mstatus >> 18) & 1;
            if sum == 0 {
                return Err(self.page_fault_cause(access_type));
            }
        }
        
        if (priv_level == PrivilegeLevel::User) && (u == 0) {
            return Err(self.page_fault_cause(access_type));
        }
        
        let mxr = (mstatus >> 19) & 1;
        
        match access_type {
            AccessType::Instruction => {
                if x == 0 { return Err(12); }
            },
            AccessType::Load => {
                if r == 1 { return Ok(()); }
                if x == 1 && mxr == 1 { return Ok(()); }
                return Err(13);
            },
            AccessType::Store => {
                if w == 0 { return Err(15); }
            }
        }
        Ok(())
    }
    
    fn update_ad_bits(&self, bus: &mut impl Bus, pte_addr: u32, pte: u32, access_type: AccessType) -> Result<bool, u32> {
        let mut new_pte = pte;
        let a = (pte >> 6) & 1;
        let d = (pte >> 7) & 1;
        
        if a == 0 {
            new_pte |= 1 << 6;
        }
        
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

    #[inline(always)]
    fn page_fault_cause(&self, access_type: AccessType) -> u32 {
        match access_type {
            AccessType::Instruction => 12,
            AccessType::Load => 13,
            AccessType::Store => 15,
        }
    }
}
