//! Region execution
//!
//! Executes compiled regions using VIRTUAL addresses.
//! All block lookups use cpu.pc (VA) directly.

use std::collections::HashMap;
use super::types::{BasicBlock, BasicBlockType, ControlFlowStructure, Page, RegionResult, CompiledRegion};
use super::super::super::Cpu;
use crate::cpu::trap::Trap;
use crate::memory::Bus;

/// Maximum loop iterations before forcing exit (prevents infinite loops in JIT)
const MAX_LOOP_ITERATIONS: u32 = 10000;

/// Execute a single basic block and return next PC (virtual address)
#[inline(always)]
fn execute_basic_block(
    cpu: &mut Cpu,
    bus: &mut impl Bus,
    block: &BasicBlock,
) -> Result<u32, Trap> {
    // Execute all instructions in the block
    for cached in &block.instructions {
        cpu.execute_cached(cached.raw, cached, bus)?;
    }
    
    // Determine next PC based on terminator (returns VA)
    match &block.ty {
        BasicBlockType::Fallthrough { next } => {
            Ok(next.unwrap_or(cpu.pc))
        }
        BasicBlockType::Jump { target, .. } => {
            Ok(target.unwrap_or(cpu.pc))
        }
        BasicBlockType::Branch { taken, not_taken, condition, .. } => {
            let take_branch = condition.evaluate(&cpu.regs);
            let target = if take_branch { *taken } else { *not_taken };
            Ok(target.unwrap_or(cpu.pc))
        }
        BasicBlockType::IndirectJump | BasicBlockType::System => {
            Ok(cpu.pc)
        }
    }
}

/// Execute structured control flow recursively
/// 
/// All addresses are VIRTUAL. Block lookups use VA directly.
fn execute_structure(
    cpu: &mut Cpu,
    bus: &mut impl Bus,
    structure: &[ControlFlowStructure],
    blocks: &HashMap<u32, BasicBlock>,
    page: Page,
) -> RegionResult {
    for node in structure {
        match node {
            ControlFlowStructure::Block(vaddr) => {
                if let Some(block) = blocks.get(vaddr) {
                    match execute_basic_block(cpu, bus, block) {
                        Ok(next_pc) => {
                            cpu.pc = next_pc;
                            // If next PC is outside this region, exit
                            if !page.contains(next_pc) || !blocks.contains_key(&next_pc) {
                                return RegionResult::Exit(next_pc);
                            }
                        }
                        Err(trap) => return RegionResult::Trap(trap),
                    }
                } else {
                    return RegionResult::Exit(cpu.pc);
                }
            }
            
            ControlFlowStructure::Dispatcher(entries) => {
                // Find matching entry point
                let pc = cpu.pc;
                if !entries.contains(&pc) {
                    return RegionResult::Exit(pc);
                }
                // Dispatcher doesn't execute anything, just validates entry
            }
            
            ControlFlowStructure::Loop(inner) => {
                // Execute loop with iteration limit
                let mut iterations = 0;
                loop {
                    match execute_structure(cpu, bus, inner, blocks, page) {
                        RegionResult::Continue(pc) => {
                            // Check if we should continue looping
                            if !page.contains(pc) || !blocks.contains_key(&pc) {
                                return RegionResult::Exit(pc);
                            }
                            // Check if PC is still in the loop structure
                            let in_loop = inner.iter().any(|s| s.all_blocks().contains(&pc));
                            if !in_loop {
                                // Exit loop but continue in region
                                cpu.pc = pc;
                                break;
                            }
                            cpu.pc = pc;
                        }
                        result @ RegionResult::Trap(_) => return result,
                        result @ RegionResult::Exit(_) => return result,
                    }
                    
                    iterations += 1;
                    if iterations >= MAX_LOOP_ITERATIONS {
                        // Safety exit - prevent infinite loops
                        return RegionResult::Exit(cpu.pc);
                    }
                }
            }
            
            ControlFlowStructure::Forward(inner) => {
                // Forward block - execute contents and continue
                match execute_structure(cpu, bus, inner, blocks, page) {
                    RegionResult::Continue(pc) => {
                        cpu.pc = pc;
                        // Continue to next structure in parent
                    }
                    result => return result,
                }
            }
        }
    }
    
    RegionResult::Continue(cpu.pc)
}

/// Execute a compiled region using structured control flow
/// 
/// All addresses are VIRTUAL. Uses cpu.pc directly for block lookups.
pub fn execute_region(
    cpu: &mut Cpu,
    bus: &mut impl Bus,
    region: &CompiledRegion,
) -> RegionResult {
    let page = if let Some(&first_entry) = region.entry_points.first() {
        Page::of(first_entry)
    } else {
        return RegionResult::Exit(cpu.pc);
    };
    
    // Verify we're at a valid entry point (use virtual address directly)
    let vaddr = cpu.pc;
    if !region.blocks.contains_key(&vaddr) {
        return RegionResult::Exit(vaddr);
    }
    
    // Execute starting from current block
    if let Some(block) = region.blocks.get(&vaddr) {
        match execute_basic_block(cpu, bus, block) {
            Ok(next_pc) => {
                cpu.pc = next_pc;
                // If next PC is outside this region, exit
                if !page.contains(next_pc) || !region.blocks.contains_key(&next_pc) {
                    return RegionResult::Exit(next_pc);
                }
                // Continue with regular structure execution
                return execute_structure(cpu, bus, &region.structure, &region.blocks, page);
            }
            Err(trap) => return RegionResult::Trap(trap),
        }
    }
    RegionResult::Exit(cpu.pc)
}

/// Simple linear execution fallback (used when structure is empty)
#[allow(dead_code)]
pub fn execute_region_linear(
    cpu: &mut Cpu,
    bus: &mut impl Bus,
    region: &CompiledRegion,
) -> RegionResult {
    let mut pc = cpu.pc;
    let mut iterations = 0;

    loop {
        // Find the block containing current PC (VA)
        let block = match region.blocks.get(&pc) {
            Some(b) => b,
            None => return RegionResult::Exit(pc),
        };

        // Execute the block
        match execute_basic_block(cpu, bus, block) {
            Ok(next_pc) => {
                pc = next_pc;
                cpu.pc = pc;
            }
            Err(trap) => return RegionResult::Trap(trap),
        }
        
        // Check for exit conditions
        match &block.ty {
            BasicBlockType::IndirectJump | BasicBlockType::System => {
                return RegionResult::Exit(cpu.pc);
            }
            _ => {}
        }
        
        iterations += 1;
        if iterations >= MAX_LOOP_ITERATIONS {
            return RegionResult::Exit(cpu.pc);
        }
    }
}
