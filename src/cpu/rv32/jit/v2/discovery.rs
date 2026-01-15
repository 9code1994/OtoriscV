//! Basic block discovery
//!
//! Discovers basic blocks using PHYSICAL addresses (PA).
//! Entry points and block addresses are all physical addresses.

use std::collections::{HashMap, HashSet, VecDeque};
use super::types::{BasicBlock, BasicBlockType, BranchCondition, Page};
use super::super::super::decode::*;
use super::super::super::icache::CachedInst;
use crate::memory::Bus;

/// Maximum instructions per basic block
const MAX_BLOCK_SIZE: usize = 64;

/// Check if an opcode terminates a basic block
#[inline(always)]
fn is_block_terminator(opcode: u8) -> bool {
    matches!(
        opcode as u32,
        OP_BRANCH | OP_JAL | OP_JALR | OP_SYSTEM
    )
}

/// Extract branch offset from BRANCH instruction
fn extract_branch_offset(inst: u32) -> i32 {
    let imm12 = ((inst >> 31) & 1) << 12;
    let imm11 = ((inst >> 7) & 1) << 11;
    let imm10_5 = ((inst >> 25) & 0x3F) << 5;
    let imm4_1 = ((inst >> 8) & 0xF) << 1;
    let imm = imm12 | imm11 | imm10_5 | imm4_1;
    // Sign extend
    ((imm as i32) << 19) >> 19
}

/// Extract JAL offset
fn extract_jal_offset(inst: u32) -> i32 {
    let imm20 = ((inst >> 31) & 1) << 20;
    let imm19_12 = ((inst >> 12) & 0xFF) << 12;
    let imm11 = ((inst >> 20) & 1) << 11;
    let imm10_1 = ((inst >> 21) & 0x3FF) << 1;
    let imm = imm20 | imm19_12 | imm11 | imm10_1;
    // Sign extend
    ((imm as i32) << 11) >> 11
}

/// Discover basic blocks starting from entry points
///
/// All addresses are VIRTUAL addresses. The bus handles VA→PA translation.
/// Returns a map from virtual address to BasicBlock.
pub fn discover_basic_blocks(
    bus: &mut impl Bus,
    page: Page,
    entry_points: &[u32],
) -> HashMap<u32, BasicBlock> {
    let mut blocks = HashMap::new();
    let mut worklist: VecDeque<u32> = entry_points.iter().copied().collect();
    let mut visited = HashSet::new();

    let page_base = page.base_addr();
    let page_end = page_base + 0x1000;

    while let Some(start_vaddr) = worklist.pop_front() {
        // Skip if already processed or outside page
        if visited.contains(&start_vaddr) || start_vaddr < page_base || start_vaddr >= page_end {
            continue;
        }
        visited.insert(start_vaddr);

        let mut instructions = Vec::with_capacity(16);
        let mut vaddr = start_vaddr;

        // Scan instructions until terminator or limit
        loop {
            // Check page boundary
            if vaddr >= page_end {
                break;
            }

            // Read instruction via bus (which handles VA→PA translation)
            let inst = bus.read32(vaddr);
            let cached = CachedInst::decode(inst);
            let opcode = cached.opcode;
            instructions.push(cached);

            let next_vaddr = vaddr + 4;
            let is_terminator = is_block_terminator(opcode);

            if is_terminator {
                // Determine block type and successors (all targets are VIRTUAL addresses)
                let block_type = match opcode as u32 {
                    OP_BRANCH => {
                        let offset = extract_branch_offset(inst);
                        // Target is computed from current VA + offset
                        let target = (vaddr as i32).wrapping_add(offset) as u32;
                        let condition = BranchCondition::from_instruction(inst)
                            .expect("Invalid branch instruction");

                        // Add successors to worklist
                        let taken = if page.contains(target) {
                            worklist.push_back(target);
                            Some(target)
                        } else {
                            None
                        };
                        let not_taken = if page.contains(next_vaddr) {
                            worklist.push_back(next_vaddr);
                            Some(next_vaddr)
                        } else {
                            None
                        };

                        BasicBlockType::Branch {
                            taken,
                            not_taken,
                            condition,
                            offset,
                        }
                    }
                    OP_JAL => {
                        let rd = ((inst >> 7) & 0x1F) as u8;
                        let offset = extract_jal_offset(inst);
                        // Target is computed from current VA + offset
                        let target = (vaddr as i32).wrapping_add(offset) as u32;

                        if rd == 0 {
                            // Unconditional jump (tail call)
                            let target_vaddr = if page.contains(target) {
                                worklist.push_back(target);
                                Some(target)
                            } else {
                                None
                            };
                            BasicBlockType::Jump {
                                target: target_vaddr,
                                offset,
                            }
                        } else {
                            // Call - return address is next instruction
                            if page.contains(next_vaddr) {
                                worklist.push_back(next_vaddr);
                            }
                            if page.contains(target) {
                                worklist.push_back(target);
                            }
                            BasicBlockType::Jump {
                                target: if page.contains(target) {
                                    Some(target)
                                } else {
                                    None
                                },
                                offset,
                            }
                        }
                    }
                    OP_JALR => BasicBlockType::IndirectJump,
                    OP_SYSTEM => BasicBlockType::System,
                    _ => unreachable!(),
                };

                let block = BasicBlock {
                    addr: start_vaddr,
                    end_addr: next_vaddr,
                    instructions,
                    ty: block_type,
                    is_entry_point: entry_points.contains(&start_vaddr),
                };
                blocks.insert(start_vaddr, block);
                break;
            }

            // Check if we've hit an existing block start
            if visited.contains(&next_vaddr) {
                let block = BasicBlock {
                    addr: start_vaddr,
                    end_addr: next_vaddr,
                    instructions,
                    ty: BasicBlockType::Fallthrough {
                        next: Some(next_vaddr),
                    },
                    is_entry_point: entry_points.contains(&start_vaddr),
                };
                blocks.insert(start_vaddr, block);
                break;
            }

            // Check block size limit
            if instructions.len() >= MAX_BLOCK_SIZE {
                if page.contains(next_vaddr) {
                    worklist.push_back(next_vaddr);
                }
                let block = BasicBlock {
                    addr: start_vaddr,
                    end_addr: next_vaddr,
                    instructions,
                    ty: BasicBlockType::Fallthrough {
                        next: if page.contains(next_vaddr) {
                            Some(next_vaddr)
                        } else {
                            None
                        },
                    },
                    is_entry_point: entry_points.contains(&start_vaddr),
                };
                blocks.insert(start_vaddr, block);
                break;
            }

            vaddr = next_vaddr;
        }
    }

    blocks
}
