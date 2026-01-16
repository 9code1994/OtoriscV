//! Sv39 MMU implementation

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
    tag: u64,
    ppn: u64,
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

pub struct Mmu64 {
    tlb: [TlbEntry; TLB_SIZE],
    tlb_generation: u32,
    tlb_hits: u64,
    tlb_misses: u64,
}

impl Mmu64 {
    pub fn new() -> Self {
        Mmu64 {
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

    pub fn translate(&mut self, vaddr: u64, access_type: AccessType, priv_level: PrivilegeLevel, bus: &mut impl Bus, satp: u64, mstatus: u64) -> Result<u64, u64> {
        let mode = (satp >> 60) & 0xF;
        
        // Debug page faults
        if std::env::var("RISCV_DEBUG").is_ok() && access_type == AccessType::Instruction {
            eprintln!("[MMU] translate: vaddr={:#018x} priv={:?} satp={:#018x} mode={}", 
                      vaddr, priv_level, satp, mode);
        }
        
        if priv_level == PrivilegeLevel::Machine || mode == 0 {
            return Ok(vaddr);
        }
        if mode != 8 {
            eprintln!("[MMU ERROR] Invalid satp mode: {} (expected 0 or 8)", mode);
            return Err(self.page_fault_cause(access_type));
        }

        if !self.is_canonical(vaddr) {
            eprintln!("[MMU ERROR] Non-canonical address: {:#018x}", vaddr);
            return Err(self.page_fault_cause(access_type));
        }

        if let Some(paddr) = self.tlb_lookup(vaddr, access_type, priv_level, mstatus) {
            return Ok(paddr);
        }

        // Sv39 page walk
        let ppn = satp & 0x0000_FFFF_FFFF_FFFF;
        let root = ppn << 12;

        let vpn = [
            (vaddr >> 12) & 0x1FF,
            (vaddr >> 21) & 0x1FF,
            (vaddr >> 30) & 0x1FF,
        ];

        let mut level: i32 = 2;
        let mut table = root;

        loop {
            let pte_addr = table + (vpn[level as usize] * 8);
            let pte = self.read_pte(bus, pte_addr)?;

            if (pte & 1) == 0 {
                return Err(self.page_fault_cause(access_type));
            }

            let r = (pte >> 1) & 1;
            let w = (pte >> 2) & 1;
            let x = (pte >> 3) & 1;
            let u = (pte >> 4) & 1;

            if r == 1 || x == 1 {
                self.check_permissions(pte, access_type, priv_level, mstatus)?;

                let ppn_all = (pte >> 10) & 0x000F_FFFF_FFFF;
                let page_shift = match level {
                    2 => 30,
                    1 => 21,
                    _ => 12,
                };

                // Superpage alignment checks
                if level == 2 && (ppn_all & 0x3FFFF) != 0 {
                    return Err(self.page_fault_cause(access_type));
                }
                if level == 1 && (ppn_all & 0x1FF) != 0 {
                    return Err(self.page_fault_cause(access_type));
                }

                let paddr = (ppn_all << 12) | (vaddr & ((1u64 << page_shift) - 1));

                if self.update_ad_bits(bus, pte_addr, pte, access_type)? {
                    // PTE updated
                }

                let perm = ((r as u8) << 0) | ((w as u8) << 1) | ((x as u8) << 2) | ((u as u8) << 3);
                let tag = vaddr >> page_shift;
                self.fill_tlb(tag, ppn_all >> (page_shift - 12), perm, page_shift as u8);

                return Ok(paddr);
            }

            level -= 1;
            if level < 0 {
                return Err(self.page_fault_cause(access_type));
            }

            let next_ppn = (pte >> 10) & 0x000F_FFFF_FFFF;
            table = next_ppn << 12;
        }
    }

    fn read_pte(&self, bus: &mut impl Bus, addr: u64) -> Result<u64, u64> {
        if addr > u32::MAX as u64 {
            return Err(self.page_fault_cause(AccessType::Load));
        }
        Ok(bus.read64(addr as u32))
    }

    fn write_pte(&self, bus: &mut impl Bus, addr: u64, value: u64) -> Result<(), u64> {
        if addr > u32::MAX as u64 {
            return Err(self.page_fault_cause(AccessType::Store));
        }
        bus.write64(addr as u32, value);
        Ok(())
    }

    fn is_canonical(&self, vaddr: u64) -> bool {
        let sign = (vaddr >> 38) & 1;
        let upper = vaddr >> 39;
        if sign == 0 {
            upper == 0
        } else {
            upper == 0x1FFFFFF
        }
    }

    fn tlb_lookup(&mut self, vaddr: u64, access_type: AccessType, priv_level: PrivilegeLevel, mstatus: u64) -> Option<u64> {
        for &page_shift in &[12u8, 21u8, 30u8] {
            let tag = vaddr >> page_shift;
            let idx = (tag as usize) & (TLB_SIZE - 1);
            if let Some(paddr) = self.tlb_match(idx, tag, page_shift, vaddr, access_type, priv_level, mstatus) {
                self.tlb_hits += 1;
                return Some(paddr);
            }
        }
        self.tlb_misses += 1;
        None
    }

    fn tlb_match(&self, idx: usize, tag: u64, page_shift: u8, vaddr: u64, access_type: AccessType, priv_level: PrivilegeLevel, mstatus: u64) -> Option<u64> {
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
        let offset_mask = (1u64 << page_shift) - 1;
        Some((entry.ppn << 12) | (vaddr & offset_mask))
    }

    fn fill_tlb(&mut self, tag: u64, ppn: u64, perm: u8, page_shift: u8) {
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

    fn check_permissions(&self, pte: u64, access_type: AccessType, priv_level: PrivilegeLevel, mstatus: u64) -> Result<(), u64> {
        let r = (pte >> 1) & 1;
        let w = (pte >> 2) & 1;
        let x = (pte >> 3) & 1;
        let u = (pte >> 4) & 1;

        if priv_level == PrivilegeLevel::Supervisor && u == 1 {
            let sum = (mstatus >> 18) & 1;
            if sum == 0 {
                return Err(self.page_fault_cause(access_type));
            }
        }

        if priv_level == PrivilegeLevel::User && u == 0 {
            return Err(self.page_fault_cause(access_type));
        }

        let mxr = (mstatus >> 19) & 1;

        match access_type {
            AccessType::Instruction => if x == 0 { return Err(12); },
            AccessType::Load => {
                if r == 1 {
                    return Ok(());
                }
                if x == 1 && mxr == 1 {
                    return Ok(());
                }
                return Err(13);
            }
            AccessType::Store => if w == 0 { return Err(15); },
        }
        Ok(())
    }

    fn check_perm_fast(&self, perm: u8, access_type: AccessType, priv_level: PrivilegeLevel, mstatus: u64) -> bool {
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

    fn update_ad_bits(&self, bus: &mut impl Bus, pte_addr: u64, pte: u64, access_type: AccessType) -> Result<bool, u64> {
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
            self.write_pte(bus, pte_addr, new_pte)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn page_fault_cause(&self, access_type: AccessType) -> u64 {
        match access_type {
            AccessType::Instruction => 12,
            AccessType::Load => 13,
            AccessType::Store => 15,
        }
    }
}

impl Default for Mmu64 {
    fn default() -> Self {
        Self::new()
    }
}
