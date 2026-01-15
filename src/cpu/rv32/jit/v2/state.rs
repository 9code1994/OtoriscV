//! JIT state management
//!
//! JIT v2 works with physical addresses (PA). All blocks and pages are keyed by PA.
//! The cache must be invalidated on:
//! - FENCE.I instruction
//! - SFENCE.VMA instruction
//! - SATP register changes (causes full invalidation)

use std::collections::{HashMap, HashSet};
use super::types::{Page, CompiledRegion};
use super::cfg::{build_cfg, find_sccs, structure_sccs};
use super::discovery::discover_basic_blocks;
use crate::memory::Bus;

/// Compilation threshold
pub const JIT_THRESHOLD: u32 = 100_000;

/// Heat added per basic block execution
pub const HEAT_PER_BLOCK: u32 = 100;

/// Per-page execution statistics
#[derive(Default)]
pub struct PageStats {
    /// Accumulated heat
    pub hotness: u32,
    /// Known entry points (page offsets)
    pub entry_points: HashSet<u16>,
}

/// JIT compilation state
/// 
/// All addresses are VIRTUAL addresses. The JIT cache is invalidated
/// when page tables change (SFENCE.VMA, SATP write).
pub struct JitState {
    /// Per-page statistics (keyed by virtual page)
    page_stats: HashMap<Page, PageStats>,
    /// Compiled regions (keyed by virtual page)
    regions: HashMap<Page, CompiledRegion>,
    /// Global generation counter
    generation: u32,
    /// Compilation threshold
    threshold: u32,
    /// Last SATP value (to detect page table changes)
    last_satp: u32,
    /// Statistics
    pub compiles: u64,
    pub region_hits: u64,
    pub region_misses: u64,
}

impl Default for JitState {
    fn default() -> Self {
        Self::new()
    }
}

impl JitState {
    pub fn new() -> Self {
        JitState {
            page_stats: HashMap::new(),
            regions: HashMap::new(),
            generation: 1,
            threshold: JIT_THRESHOLD,
            last_satp: 0,
            compiles: 0,
            region_hits: 0,
            region_misses: 0,
        }
    }

    /// Set compilation threshold
    pub fn set_threshold(&mut self, threshold: u32) {
        self.threshold = threshold;
    }

    /// Check if SATP changed and invalidate if needed
    #[inline]
    pub fn check_satp(&mut self, satp: u32) {
        if satp != self.last_satp {
            self.invalidate_all();
            self.last_satp = satp;
        }
    }

    /// Record execution at a virtual address and return page if compilation should be triggered
    #[inline]
    pub fn record_execution(&mut self, vaddr: u32, heat: u32) -> Option<Page> {
        let page = Page::of(vaddr);
        let offset = (vaddr & 0xFFF) as u16;

        let stats = self.page_stats.entry(page).or_default();
        stats.entry_points.insert(offset);
        stats.hotness += heat;

        if stats.hotness >= self.threshold {
            stats.hotness = 0;
            Some(page)
        } else {
            None
        }
    }

    /// Get compiled region for a virtual page
    #[inline]
    pub fn get_region(&mut self, page: Page) -> Option<&CompiledRegion> {
        if let Some(region) = self.regions.get(&page) {
            if region.generation == self.generation {
                self.region_hits += 1;
                return Some(region);
            }
        }
        self.region_misses += 1;
        None
    }

    /// Compile a region for the given virtual page
    /// 
    /// The bus is used to read instructions. Since we're using virtual addresses,
    /// the bus must handle VA→PA translation (which SystemBus does via MMU).
    pub fn compile_region(&mut self, bus: &mut impl Bus, page: Page) {
        let entry_points: Vec<u32> = self
            .page_stats
            .get(&page)
            .map(|stats| {
                stats
                    .entry_points
                    .iter()
                    .map(|&offset| page.base_addr() + offset as u32)
                    .collect()
            })
            .unwrap_or_else(|| vec![page.base_addr()]);

        // Discover basic blocks (using virtual addresses)
        let blocks = discover_basic_blocks(bus, page, &entry_points);

        if blocks.is_empty() {
            return;
        }

        // Build CFG
        let blocks_vec: Vec<_> = blocks.values().cloned().collect();
        let cfg = build_cfg(&blocks_vec);

        // Find SCCs
        let sccs = find_sccs(&cfg);

        // Structure control flow
        let structure = structure_sccs(&cfg, &sccs, &entry_points);

        // Store compiled region
        let region = CompiledRegion {
            blocks,
            structure,
            entry_points,
            generation: self.generation,
        };

        self.regions.insert(page, region);
        self.compiles += 1;
    }

    /// Invalidate all compiled regions (called on SFENCE.VMA, SATP change)
    pub fn invalidate_all(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.generation = 1;
        }
    }

    /// Invalidate a specific virtual page
    pub fn invalidate_page(&mut self, page: Page) {
        self.regions.remove(&page);
        self.page_stats.remove(&page);
    }

    /// Reset the JIT state
    pub fn reset(&mut self) {
        self.page_stats.clear();
        self.regions.clear();
        self.generation = 1;
        self.last_satp = 0;
        self.compiles = 0;
        self.region_hits = 0;
        self.region_misses = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jit_state_hotness() {
        let mut jit = JitState::new();
        jit.set_threshold(1000);

        // Record executions
        for _ in 0..9 {
            assert!(jit.record_execution(0x8000_0000, 100).is_none());
        }

        // 10th execution should trigger compilation
        assert!(jit.record_execution(0x8000_0000, 100).is_some());
    }
}
