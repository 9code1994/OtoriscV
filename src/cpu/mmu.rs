//! Sv32 MMU implementation
//!
//! Handles virtual address translation for 32-bit RISC-V.
//! Supports 4KB pages and 4MB megapages.

use crate::memory::Bus;
use super::PrivilegeLevel;

/// Access type for translation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    Instruction,
    Load,
    Store,
}

/// MMU State
#[derive(Default)] // We'll add Serialize/Deserialize later
pub struct Mmu {
    /// Helper to store effective privilege mode if different from CPU priv
    /// (e.g. MPRV bit in MSTATUS) - typically handled by caller passing correct priv
    _dummy: u32, 
}

impl Mmu {
    pub fn new() -> Self {
        Mmu { _dummy: 0 }
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

        // 2. Sv32 Translation
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
        let x0 = (pte0 >> 3) & 1;
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

        Ok(paddr)
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
