//! Sv32 MMU implementation
//!
//! Handles virtual address translation for 32-bit RISC-V.
//! Supports 4KB pages and 4MB megapages.
//! Uses ultra-simple single-entry TLBs per access type (like jor1k).

use crate::cpu::PrivilegeLevel;
use crate::memory::Bus;

/// Access type for translation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    Instruction,
    Load,
    Store,
}

/// Ultra-simple single-entry TLB per access type
/// Stores the VPN and pre-computed physical page base
#[derive(Clone, Copy)]
struct SimpleTlbEntry {
    /// Virtual page number (vaddr >> page_shift)
    vpn: u32,
    /// Physical address base (ppn << page_shift), XORed to allow direct use
    pa_base: u32,
    /// Page mask (0xFFF for 4KB, 0x3FFFFF for 4MB)
    offset_mask: u32,
    /// Valid flag
    valid: bool,
}

impl SimpleTlbEntry {
    const fn empty() -> Self {
        SimpleTlbEntry {
            vpn: 0,
            pa_base: 0,
            offset_mask: 0,
            valid: false,
        }
    }
}

/// MMU State with ultra-fast TLB
pub struct Mmu {
    /// Single TLB entry for instruction fetch
    itlb: SimpleTlbEntry,
    /// Single TLB entry for loads  
    dtlb_read: SimpleTlbEntry,
    /// Single TLB entry for stores
    dtlb_write: SimpleTlbEntry,
    
    /// Generation for lazy invalidation
    generation: u32,
    itlb_gen: u32,
    dtlb_read_gen: u32,
    dtlb_write_gen: u32,
    
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
            itlb: SimpleTlbEntry::empty(),
            dtlb_read: SimpleTlbEntry::empty(),
            dtlb_write: SimpleTlbEntry::empty(),
            generation: 1,
            itlb_gen: 0,
            dtlb_read_gen: 0,
            dtlb_write_gen: 0,
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

    /// Ultra-fast translation with inline TLB check
    #[inline(always)]
    pub fn translate(&mut self, vaddr: u32, access_type: AccessType, priv_level: PrivilegeLevel, bus: &mut impl Bus, satp: u32, mstatus: u32) -> Result<u32, u32> {
        // Fast path: Machine mode or paging disabled
        let mode = satp >> 31;
        if priv_level == PrivilegeLevel::Machine || mode == 0 {
            return Ok(vaddr);
        }

        // TLB lookup based on access type
        match access_type {
            AccessType::Instruction => {
                if self.itlb_gen == self.generation && self.itlb.valid {
                    let vpn = vaddr & !self.itlb.offset_mask;
                    if vpn == self.itlb.vpn {
                        self.tlb_hits += 1;
                        return Ok(self.itlb.pa_base | (vaddr & self.itlb.offset_mask));
                    }
                }
            }
            AccessType::Load => {
                if self.dtlb_read_gen == self.generation && self.dtlb_read.valid {
                    let vpn = vaddr & !self.dtlb_read.offset_mask;
                    if vpn == self.dtlb_read.vpn {
                        self.tlb_hits += 1;
                        return Ok(self.dtlb_read.pa_base | (vaddr & self.dtlb_read.offset_mask));
                    }
                }
            }
            AccessType::Store => {
                if self.dtlb_write_gen == self.generation && self.dtlb_write.valid {
                    let vpn = vaddr & !self.dtlb_write.offset_mask;
                    if vpn == self.dtlb_write.vpn {
                        self.tlb_hits += 1;
                        return Ok(self.dtlb_write.pa_base | (vaddr & self.dtlb_write.offset_mask));
                    }
                }
            }
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
            
            // Fill TLB with megapage entry
            let entry = SimpleTlbEntry {
                vpn: vaddr & 0xFFC0_0000, // 4MB aligned
                pa_base,
                offset_mask: 0x003F_FFFF, // 4MB - 1
                valid: true,
            };
            self.fill_tlb(access_type, entry);
            
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

        // Fill TLB with 4KB entry
        let entry = SimpleTlbEntry {
            vpn: vaddr & 0xFFFF_F000, // 4KB aligned
            pa_base,
            offset_mask: 0x0000_0FFF, // 4KB - 1
            valid: true,
        };
        self.fill_tlb(access_type, entry);

        Ok(paddr)
    }

    #[inline(always)]
    fn fill_tlb(&mut self, access_type: AccessType, entry: SimpleTlbEntry) {
        match access_type {
            AccessType::Instruction => {
                self.itlb = entry;
                self.itlb_gen = self.generation;
            }
            AccessType::Load => {
                self.dtlb_read = entry;
                self.dtlb_read_gen = self.generation;
            }
            AccessType::Store => {
                self.dtlb_write = entry;
                self.dtlb_write_gen = self.generation;
            }
        }
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
