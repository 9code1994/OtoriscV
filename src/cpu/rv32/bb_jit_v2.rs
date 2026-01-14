//! BB-JIT V2: Advanced Basic Block JIT with CFG Optimization
//!
//! Inspired by v86's JIT architecture:
//! - Hotness-based compilation
//! - Control flow graph analysis with SCC detection
//! - Structured control flow for efficient code generation
//! - Multi-entry point dispatching

use std::collections::{HashMap, HashSet, VecDeque};

use super::decode::*;
use super::icache::CachedInst;
use super::Cpu;
use crate::cpu::trap::Trap;
use crate::memory::Bus;

// =============================================================================
// Core Types
// =============================================================================

/// Physical page identifier (upper 20 bits of address)
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct Page(u32);

impl Page {
    #[inline(always)]
    pub fn of(addr: u32) -> Self {
        Page(addr >> 12)
    }

    #[inline(always)]
    pub fn base_addr(self) -> u32 {
        self.0 << 12
    }

    #[inline(always)]
    pub fn contains(self, addr: u32) -> bool {
        Page::of(addr) == self
    }
}

/// Result of executing a compiled region
#[derive(Debug)]
pub enum RegionResult {
    /// Continue execution at the given PC
    Continue(u32),
    /// A trap occurred
    Trap(Trap),
    /// Exit region (indirect jump, system call, etc.)
    Exit(u32),
}

// =============================================================================
// Basic Block Types
// =============================================================================

/// Branch condition extracted from instruction
#[derive(Clone, Debug)]
pub enum BranchCondition {
    /// BEQ rs1, rs2
    Eq { rs1: u8, rs2: u8 },
    /// BNE rs1, rs2
    Ne { rs1: u8, rs2: u8 },
    /// BLT rs1, rs2 (signed)
    Lt { rs1: u8, rs2: u8 },
    /// BGE rs1, rs2 (signed)
    Ge { rs1: u8, rs2: u8 },
    /// BLTU rs1, rs2 (unsigned)
    Ltu { rs1: u8, rs2: u8 },
    /// BGEU rs1, rs2 (unsigned)
    Geu { rs1: u8, rs2: u8 },
}

impl BranchCondition {
    /// Evaluate the condition against CPU register values
    #[inline(always)]
    pub fn evaluate(&self, regs: &[u32; 32]) -> bool {
        match self {
            Self::Eq { rs1, rs2 } => regs[*rs1 as usize] == regs[*rs2 as usize],
            Self::Ne { rs1, rs2 } => regs[*rs1 as usize] != regs[*rs2 as usize],
            Self::Lt { rs1, rs2 } => {
                (regs[*rs1 as usize] as i32) < (regs[*rs2 as usize] as i32)
            }
            Self::Ge { rs1, rs2 } => {
                (regs[*rs1 as usize] as i32) >= (regs[*rs2 as usize] as i32)
            }
            Self::Ltu { rs1, rs2 } => regs[*rs1 as usize] < regs[*rs2 as usize],
            Self::Geu { rs1, rs2 } => regs[*rs1 as usize] >= regs[*rs2 as usize],
        }
    }

    /// Extract branch condition from BRANCH instruction
    pub fn from_instruction(inst: u32) -> Option<Self> {
        let funct3 = ((inst >> 12) & 0x7) as u8;
        let rs1 = ((inst >> 15) & 0x1F) as u8;
        let rs2 = ((inst >> 20) & 0x1F) as u8;

        match funct3 {
            0b000 => Some(Self::Eq { rs1, rs2 }),  // BEQ
            0b001 => Some(Self::Ne { rs1, rs2 }),  // BNE
            0b100 => Some(Self::Lt { rs1, rs2 }),  // BLT
            0b101 => Some(Self::Ge { rs1, rs2 }),  // BGE
            0b110 => Some(Self::Ltu { rs1, rs2 }), // BLTU
            0b111 => Some(Self::Geu { rs1, rs2 }), // BGEU
            _ => None,
        }
    }
}

/// Basic block terminator type
#[derive(Clone, Debug)]
pub enum BasicBlockType {
    /// Fallthrough to next instruction (non-control-flow terminator or block size limit)
    Fallthrough {
        /// Physical address of next block (if in same page)
        next: Option<u32>,
    },
    /// Unconditional jump (JAL with rd=x0)
    Jump {
        /// Target physical address (if static and in same page)
        target: Option<u32>,
        /// Jump offset for recalculation
        offset: i32,
    },
    /// Conditional branch
    Branch {
        /// Target if branch taken
        taken: Option<u32>,
        /// Target if branch not taken (next instruction)
        not_taken: Option<u32>,
        /// Branch condition
        condition: BranchCondition,
        /// Branch offset
        offset: i32,
    },
    /// Indirect jump (JALR, computed gotos, returns)
    IndirectJump,
    /// System instruction (ECALL, EBREAK, CSR, etc.) - always exits
    System,
}

/// A basic block within a compiled region
#[derive(Clone)]
pub struct BasicBlock {
    /// Physical start address
    pub addr: u32,
    /// Physical end address (exclusive - address after last instruction)
    pub end_addr: u32,
    /// Decoded instructions
    pub instructions: Vec<CachedInst>,
    /// Terminator type with control flow edges
    pub ty: BasicBlockType,
    /// Is this an entry point from outside the region?
    pub is_entry_point: bool,
}

impl BasicBlock {
    /// Get successor addresses
    pub fn successors(&self) -> Vec<u32> {
        match &self.ty {
            BasicBlockType::Fallthrough { next: Some(addr) } => vec![*addr],
            BasicBlockType::Jump { target: Some(addr), .. } => vec![*addr],
            BasicBlockType::Branch {
                taken,
                not_taken,
                ..
            } => {
                let mut succs = Vec::with_capacity(2);
                if let Some(addr) = taken {
                    succs.push(*addr);
                }
                if let Some(addr) = not_taken {
                    succs.push(*addr);
                }
                succs
            }
            _ => vec![],
        }
    }
}

// =============================================================================
// Control Flow Graph
// =============================================================================

/// Control flow graph represented as adjacency list
pub type CfgGraph = HashMap<u32, HashSet<u32>>;

/// Build CFG from basic blocks
pub fn build_cfg(blocks: &[BasicBlock]) -> CfgGraph {
    let mut graph = CfgGraph::new();

    for block in blocks {
        let successors: HashSet<u32> = block.successors().into_iter().collect();
        graph.insert(block.addr, successors);
    }

    graph
}

/// Reverse all edges in a graph
fn reverse_graph(graph: &CfgGraph) -> CfgGraph {
    let mut rev = CfgGraph::new();

    // Ensure all nodes exist in reverse graph
    for &node in graph.keys() {
        rev.entry(node).or_default();
    }

    // Add reversed edges
    for (&from, tos) in graph {
        for &to in tos {
            rev.entry(to).or_default().insert(from);
        }
    }

    rev
}

/// Find strongly connected components using Kosaraju's algorithm
///
/// Returns SCCs in reverse topological order (leaves first)
pub fn find_sccs(graph: &CfgGraph) -> Vec<Vec<u32>> {
    // Phase 1: DFS to get finish order
    let mut visited = HashSet::new();
    let mut finish_order = Vec::new();

    fn dfs_finish(
        node: u32,
        graph: &CfgGraph,
        visited: &mut HashSet<u32>,
        finish_order: &mut Vec<u32>,
    ) {
        if visited.contains(&node) {
            return;
        }
        visited.insert(node);

        if let Some(successors) = graph.get(&node) {
            for &succ in successors {
                dfs_finish(succ, graph, visited, finish_order);
            }
        }

        finish_order.push(node);
    }

    for &node in graph.keys() {
        dfs_finish(node, graph, &mut visited, &mut finish_order);
    }

    // Phase 2: DFS on reverse graph in reverse finish order
    let rev_graph = reverse_graph(graph);
    let mut visited = HashSet::new();
    let mut sccs = Vec::new();

    fn dfs_collect(
        node: u32,
        rev_graph: &CfgGraph,
        visited: &mut HashSet<u32>,
        component: &mut Vec<u32>,
    ) {
        if visited.contains(&node) {
            return;
        }
        visited.insert(node);
        component.push(node);

        if let Some(predecessors) = rev_graph.get(&node) {
            for &pred in predecessors {
                dfs_collect(pred, rev_graph, visited, component);
            }
        }
    }

    for &node in finish_order.iter().rev() {
        if !visited.contains(&node) {
            let mut component = Vec::new();
            dfs_collect(node, &rev_graph, &mut visited, &mut component);
            if !component.is_empty() {
                sccs.push(component);
            }
        }
    }

    sccs
}

// =============================================================================
// Structured Control Flow
// =============================================================================

/// Structured control flow for code generation
/// Mirrors v86's WasmStructure
#[derive(Clone, Debug)]
pub enum ControlFlowStructure {
    /// Single basic block
    Block(u32),
    /// Entry point dispatcher (br_table equivalent)
    Dispatcher(Vec<u32>),
    /// Loop construct (for back-edges)
    Loop(Vec<ControlFlowStructure>),
    /// Block construct (for forward jumps)
    Forward(Vec<ControlFlowStructure>),
}

impl ControlFlowStructure {
    /// Get the "head" addresses of this structure
    pub fn head(&self) -> Vec<u32> {
        match self {
            Self::Block(addr) => vec![*addr],
            Self::Dispatcher(entries) => entries.clone(),
            Self::Loop(children) | Self::Forward(children) => {
                children.first().map(|c| c.head()).unwrap_or_default()
            }
        }
    }

    /// Get all block addresses in this structure
    pub fn all_blocks(&self) -> Vec<u32> {
        match self {
            Self::Block(addr) => vec![*addr],
            Self::Dispatcher(_) => vec![],
            Self::Loop(children) | Self::Forward(children) => {
                children.iter().flat_map(|c| c.all_blocks()).collect()
            }
        }
    }
}

/// Convert SCCs to structured control flow
///
/// This is a simplified version of v86's loopify/blockify
pub fn structure_sccs(graph: &CfgGraph, sccs: &[Vec<u32>], entry_points: &[u32]) -> Vec<ControlFlowStructure> {
    let mut result = Vec::new();

    // Add dispatcher if multiple entry points
    if entry_points.len() > 1 {
        result.push(ControlFlowStructure::Dispatcher(entry_points.to_vec()));
    }

    for scc in sccs {
        if scc.is_empty() {
            continue;
        }

        if scc.len() == 1 {
            let addr = scc[0];
            // Check for self-loop
            let is_self_loop = graph
                .get(&addr)
                .map_or(false, |succs| succs.contains(&addr));

            if is_self_loop {
                result.push(ControlFlowStructure::Loop(vec![
                    ControlFlowStructure::Block(addr),
                ]));
            } else {
                result.push(ControlFlowStructure::Block(addr));
            }
        } else {
            // Multi-block SCC = loop
            // For simplicity, we just wrap all blocks in a loop
            // A more sophisticated implementation would recursively structure
            let inner: Vec<ControlFlowStructure> = scc
                .iter()
                .map(|&addr| ControlFlowStructure::Block(addr))
                .collect();
            result.push(ControlFlowStructure::Loop(inner));
        }
    }

    result
}

// =============================================================================
// Basic Block Discovery
// =============================================================================

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
/// Returns a map from physical address to BasicBlock
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

    while let Some(start_addr) = worklist.pop_front() {
        // Skip if already processed or outside page
        if visited.contains(&start_addr) || start_addr < page_base || start_addr >= page_end {
            continue;
        }
        visited.insert(start_addr);

        let mut instructions = Vec::with_capacity(16);
        let mut addr = start_addr;

        // Scan instructions until terminator or limit
        loop {
            // Check page boundary
            if addr >= page_end {
                break;
            }

            let inst = bus.read32(addr);
            let cached = CachedInst::decode(inst);
            let opcode = cached.opcode;
            instructions.push(cached);

            let next_addr = addr + 4;
            let is_terminator = is_block_terminator(opcode);

            if is_terminator {
                // Determine block type and successors
                let block_type = match opcode as u32 {
                    OP_BRANCH => {
                        let offset = extract_branch_offset(inst);
                        let target = (addr as i32).wrapping_add(offset) as u32;
                        let condition = BranchCondition::from_instruction(inst)
                            .expect("Invalid branch instruction");

                        // Add successors to worklist
                        let taken = if page.contains(target) {
                            worklist.push_back(target);
                            Some(target)
                        } else {
                            None
                        };
                        let not_taken = if page.contains(next_addr) {
                            worklist.push_back(next_addr);
                            Some(next_addr)
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
                        let target = (addr as i32).wrapping_add(offset) as u32;

                        if rd == 0 {
                            // Unconditional jump (tail call)
                            let target_addr = if page.contains(target) {
                                worklist.push_back(target);
                                Some(target)
                            } else {
                                None
                            };
                            BasicBlockType::Jump {
                                target: target_addr,
                                offset,
                            }
                        } else {
                            // Call - return address is next instruction
                            if page.contains(next_addr) {
                                worklist.push_back(next_addr);
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
                    addr: start_addr,
                    end_addr: next_addr,
                    instructions,
                    ty: block_type,
                    is_entry_point: entry_points.contains(&start_addr),
                };
                blocks.insert(start_addr, block);
                break;
            }

            // Check if we've hit an existing block start
            if visited.contains(&next_addr) {
                let block = BasicBlock {
                    addr: start_addr,
                    end_addr: next_addr,
                    instructions,
                    ty: BasicBlockType::Fallthrough {
                        next: Some(next_addr),
                    },
                    is_entry_point: entry_points.contains(&start_addr),
                };
                blocks.insert(start_addr, block);
                break;
            }

            // Check block size limit
            if instructions.len() >= MAX_BLOCK_SIZE {
                if page.contains(next_addr) {
                    worklist.push_back(next_addr);
                }
                let block = BasicBlock {
                    addr: start_addr,
                    end_addr: next_addr,
                    instructions,
                    ty: BasicBlockType::Fallthrough {
                        next: if page.contains(next_addr) {
                            Some(next_addr)
                        } else {
                            None
                        },
                    },
                    is_entry_point: entry_points.contains(&start_addr),
                };
                blocks.insert(start_addr, block);
                break;
            }

            addr = next_addr;
        }
    }

    blocks
}

// =============================================================================
// JIT State
// =============================================================================

/// Compilation threshold (v86 uses 200_000, we use lower for faster warmup)
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

/// Compiled region for a page
pub struct CompiledRegion {
    /// Basic blocks in this region
    pub blocks: HashMap<u32, BasicBlock>,
    /// Structured control flow
    pub structure: Vec<ControlFlowStructure>,
    /// Entry points
    pub entry_points: Vec<u32>,
    /// Generation counter for invalidation
    pub generation: u32,
}

/// JIT compilation state
pub struct JitState {
    /// Per-page statistics
    page_stats: HashMap<Page, PageStats>,
    /// Compiled regions
    regions: HashMap<Page, CompiledRegion>,
    /// Global generation counter
    generation: u32,
    /// Compilation threshold
    threshold: u32,
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
            compiles: 0,
            region_hits: 0,
            region_misses: 0,
        }
    }

    /// Set compilation threshold
    pub fn set_threshold(&mut self, threshold: u32) {
        self.threshold = threshold;
    }

    /// Record execution and return page if compilation should be triggered
    #[inline]
    pub fn record_execution(&mut self, paddr: u32, heat: u32) -> Option<Page> {
        let page = Page::of(paddr);
        let offset = (paddr & 0xFFF) as u16;

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

    /// Get compiled region for a page
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

    /// Compile a region for the given page
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

        // Discover basic blocks
        let blocks = discover_basic_blocks(bus, page, &entry_points);

        if blocks.is_empty() {
            return;
        }

        // Build CFG
        let blocks_vec: Vec<BasicBlock> = blocks.values().cloned().collect();
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

    /// Invalidate all compiled regions
    pub fn invalidate_all(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.generation = 1;
        }
    }

    /// Invalidate a specific page
    pub fn invalidate_page(&mut self, page: Page) {
        self.regions.remove(&page);
        self.page_stats.remove(&page);
    }

    /// Reset the JIT state
    pub fn reset(&mut self) {
        self.page_stats.clear();
        self.regions.clear();
        self.generation = 1;
        self.compiles = 0;
        self.region_hits = 0;
        self.region_misses = 0;
    }
}

// =============================================================================
// Region Execution
// =============================================================================

/// Maximum loop iterations before forcing exit (prevents infinite loops in JIT)
const MAX_LOOP_ITERATIONS: u32 = 10000;

/// Execute a single basic block and return next PC
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
    
    // Determine next PC based on terminator
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
/// This is the native code generation equivalent - we execute the structured
/// control flow directly in Rust, allowing the compiler to optimize loops.
fn execute_structure(
    cpu: &mut Cpu,
    bus: &mut impl Bus,
    structure: &[ControlFlowStructure],
    blocks: &HashMap<u32, BasicBlock>,
    page: Page,
) -> RegionResult {
    for node in structure {
        match node {
            ControlFlowStructure::Block(addr) => {
                if let Some(block) = blocks.get(addr) {
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
    
    // Verify we're at a valid entry point
    let pc = cpu.pc;
    if !region.blocks.contains_key(&pc) {
        return RegionResult::Exit(pc);
    }
    
    // Execute structured control flow
    execute_structure(cpu, bus, &region.structure, &region.blocks, page)
}

/// Simple linear execution fallback (used when structure is empty)
pub fn execute_region_linear(
    cpu: &mut Cpu,
    bus: &mut impl Bus,
    region: &CompiledRegion,
) -> RegionResult {
    let mut pc = cpu.pc;
    let mut iterations = 0;

    loop {
        // Find the block containing current PC
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

// =============================================================================
// WASM Code Generation (Phase 5 - for wasm32 target)
// =============================================================================

#[cfg(target_arch = "wasm32")]
pub mod wasm_codegen {
    //! WebAssembly code generation for JIT compiled regions
    //! 
    //! This module generates WASM bytecode for compiled regions,
    //! allowing direct execution via the browser's WASM engine.
    
    /// WebAssembly opcodes
    pub mod op {
        pub const OP_UNREACHABLE: u8 = 0x00;
        pub const OP_NOP: u8 = 0x01;
        pub const OP_BLOCK: u8 = 0x02;
        pub const OP_LOOP: u8 = 0x03;
        pub const OP_IF: u8 = 0x04;
        pub const OP_ELSE: u8 = 0x05;
        pub const OP_END: u8 = 0x0B;
        pub const OP_BR: u8 = 0x0C;
        pub const OP_BR_IF: u8 = 0x0D;
        pub const OP_BR_TABLE: u8 = 0x0E;
        pub const OP_RETURN: u8 = 0x0F;
        pub const OP_CALL: u8 = 0x10;
        pub const OP_CALL_INDIRECT: u8 = 0x11;
        
        pub const OP_DROP: u8 = 0x1A;
        pub const OP_SELECT: u8 = 0x1B;
        
        pub const OP_LOCAL_GET: u8 = 0x20;
        pub const OP_LOCAL_SET: u8 = 0x21;
        pub const OP_LOCAL_TEE: u8 = 0x22;
        pub const OP_GLOBAL_GET: u8 = 0x23;
        pub const OP_GLOBAL_SET: u8 = 0x24;
        
        pub const OP_I32_LOAD: u8 = 0x28;
        pub const OP_I64_LOAD: u8 = 0x29;
        pub const OP_I32_LOAD8_S: u8 = 0x2C;
        pub const OP_I32_LOAD8_U: u8 = 0x2D;
        pub const OP_I32_LOAD16_S: u8 = 0x2E;
        pub const OP_I32_LOAD16_U: u8 = 0x2F;
        pub const OP_I32_STORE: u8 = 0x36;
        pub const OP_I32_STORE8: u8 = 0x3A;
        pub const OP_I32_STORE16: u8 = 0x3B;
        
        pub const OP_I32_CONST: u8 = 0x41;
        pub const OP_I64_CONST: u8 = 0x42;
        
        pub const OP_I32_EQZ: u8 = 0x45;
        pub const OP_I32_EQ: u8 = 0x46;
        pub const OP_I32_NE: u8 = 0x47;
        pub const OP_I32_LT_S: u8 = 0x48;
        pub const OP_I32_LT_U: u8 = 0x49;
        pub const OP_I32_GT_S: u8 = 0x4A;
        pub const OP_I32_GT_U: u8 = 0x4B;
        pub const OP_I32_LE_S: u8 = 0x4C;
        pub const OP_I32_LE_U: u8 = 0x4D;
        pub const OP_I32_GE_S: u8 = 0x4E;
        pub const OP_I32_GE_U: u8 = 0x4F;
        
        pub const OP_I32_CLZ: u8 = 0x67;
        pub const OP_I32_CTZ: u8 = 0x68;
        pub const OP_I32_POPCNT: u8 = 0x69;
        pub const OP_I32_ADD: u8 = 0x6A;
        pub const OP_I32_SUB: u8 = 0x6B;
        pub const OP_I32_MUL: u8 = 0x6C;
        pub const OP_I32_DIV_S: u8 = 0x6D;
        pub const OP_I32_DIV_U: u8 = 0x6E;
        pub const OP_I32_REM_S: u8 = 0x6F;
        pub const OP_I32_REM_U: u8 = 0x70;
        pub const OP_I32_AND: u8 = 0x71;
        pub const OP_I32_OR: u8 = 0x72;
        pub const OP_I32_XOR: u8 = 0x73;
        pub const OP_I32_SHL: u8 = 0x74;
        pub const OP_I32_SHR_S: u8 = 0x75;
        pub const OP_I32_SHR_U: u8 = 0x76;
        pub const OP_I32_ROTL: u8 = 0x77;
        pub const OP_I32_ROTR: u8 = 0x78;
        
        pub const TYPE_I32: u8 = 0x7F;
        pub const TYPE_I64: u8 = 0x7E;
        pub const TYPE_F32: u8 = 0x7D;
        pub const TYPE_F64: u8 = 0x7C;
        pub const TYPE_VOID: u8 = 0x40;
    }
    
    use std::collections::HashMap;
    
    /// Label for structured control flow
    #[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
    pub struct Label(u32);
    
    /// WebAssembly module builder
    pub struct WasmBuilder {
        /// Output bytecode for function body
        code: Vec<u8>,
        /// Label stack for control flow
        label_stack: Vec<Label>,
        /// Next label ID
        next_label: u32,
        /// Label to stack depth mapping
        label_depths: HashMap<Label, usize>,
        /// Number of local variables
        local_count: u32,
    }
    
    impl WasmBuilder {
        pub fn new() -> Self {
            WasmBuilder {
                code: Vec::with_capacity(4096),
                label_stack: Vec::new(),
                next_label: 0,
                label_depths: HashMap::new(),
                local_count: 0,
            }
        }
        
        /// Reset builder for new function
        pub fn reset(&mut self) {
            self.code.clear();
            self.label_stack.clear();
            self.next_label = 0;
            self.label_depths.clear();
            self.local_count = 0;
        }
        
        /// Get generated code
        pub fn get_code(&self) -> &[u8] {
            &self.code
        }
        
        /// Allocate a new local variable
        pub fn alloc_local(&mut self) -> u32 {
            let idx = self.local_count;
            self.local_count += 1;
            idx
        }
        
        // === Control Flow ===
        
        /// Begin a block (forward jump target)
        pub fn block_void(&mut self) -> Label {
            let label = Label(self.next_label);
            self.next_label += 1;
            self.label_depths.insert(label, self.label_stack.len());
            self.label_stack.push(label);
            self.code.push(op::OP_BLOCK);
            self.code.push(op::TYPE_VOID);
            label
        }
        
        /// Begin a loop (backward jump target)
        pub fn loop_void(&mut self) -> Label {
            let label = Label(self.next_label);
            self.next_label += 1;
            self.label_depths.insert(label, self.label_stack.len());
            self.label_stack.push(label);
            self.code.push(op::OP_LOOP);
            self.code.push(op::TYPE_VOID);
            label
        }
        
        /// End a block or loop
        pub fn end(&mut self) {
            self.label_stack.pop();
            self.code.push(op::OP_END);
        }
        
        /// Unconditional branch
        pub fn br(&mut self, label: Label) {
            let depth = self.label_depth(label);
            self.code.push(op::OP_BR);
            self.write_leb128_u32(depth);
        }
        
        /// Conditional branch (if top of stack is non-zero)
        pub fn br_if(&mut self, label: Label) {
            let depth = self.label_depth(label);
            self.code.push(op::OP_BR_IF);
            self.write_leb128_u32(depth);
        }
        
        /// Branch table (switch)
        pub fn br_table(&mut self, labels: &[Label], default: Label) {
            self.code.push(op::OP_BR_TABLE);
            self.write_leb128_u32(labels.len() as u32);
            for &label in labels {
                let depth = self.label_depth(label);
                self.write_leb128_u32(depth);
            }
            let default_depth = self.label_depth(default);
            self.write_leb128_u32(default_depth);
        }
        
        /// If-then (condition on stack)
        pub fn if_void(&mut self) -> Label {
            let label = Label(self.next_label);
            self.next_label += 1;
            self.label_depths.insert(label, self.label_stack.len());
            self.label_stack.push(label);
            self.code.push(op::OP_IF);
            self.code.push(op::TYPE_VOID);
            label
        }
        
        /// Else branch
        pub fn else_(&mut self) {
            self.code.push(op::OP_ELSE);
        }
        
        /// Return from function
        pub fn return_(&mut self) {
            self.code.push(op::OP_RETURN);
        }
        
        // === Local Variables ===
        
        /// Get local variable
        pub fn local_get(&mut self, idx: u32) {
            self.code.push(op::OP_LOCAL_GET);
            self.write_leb128_u32(idx);
        }
        
        /// Set local variable
        pub fn local_set(&mut self, idx: u32) {
            self.code.push(op::OP_LOCAL_SET);
            self.write_leb128_u32(idx);
        }
        
        /// Tee local variable (set and keep on stack)
        pub fn local_tee(&mut self, idx: u32) {
            self.code.push(op::OP_LOCAL_TEE);
            self.write_leb128_u32(idx);
        }
        
        // === Constants ===
        
        /// Push i32 constant
        pub fn i32_const(&mut self, value: i32) {
            self.code.push(op::OP_I32_CONST);
            self.write_leb128_i32(value);
        }
        
        // === Arithmetic ===
        
        pub fn i32_add(&mut self) { self.code.push(op::OP_I32_ADD); }
        pub fn i32_sub(&mut self) { self.code.push(op::OP_I32_SUB); }
        pub fn i32_mul(&mut self) { self.code.push(op::OP_I32_MUL); }
        pub fn i32_div_s(&mut self) { self.code.push(op::OP_I32_DIV_S); }
        pub fn i32_div_u(&mut self) { self.code.push(op::OP_I32_DIV_U); }
        pub fn i32_rem_s(&mut self) { self.code.push(op::OP_I32_REM_S); }
        pub fn i32_rem_u(&mut self) { self.code.push(op::OP_I32_REM_U); }
        pub fn i32_and(&mut self) { self.code.push(op::OP_I32_AND); }
        pub fn i32_or(&mut self) { self.code.push(op::OP_I32_OR); }
        pub fn i32_xor(&mut self) { self.code.push(op::OP_I32_XOR); }
        pub fn i32_shl(&mut self) { self.code.push(op::OP_I32_SHL); }
        pub fn i32_shr_s(&mut self) { self.code.push(op::OP_I32_SHR_S); }
        pub fn i32_shr_u(&mut self) { self.code.push(op::OP_I32_SHR_U); }
        
        // === Comparison ===
        
        pub fn i32_eqz(&mut self) { self.code.push(op::OP_I32_EQZ); }
        pub fn i32_eq(&mut self) { self.code.push(op::OP_I32_EQ); }
        pub fn i32_ne(&mut self) { self.code.push(op::OP_I32_NE); }
        pub fn i32_lt_s(&mut self) { self.code.push(op::OP_I32_LT_S); }
        pub fn i32_lt_u(&mut self) { self.code.push(op::OP_I32_LT_U); }
        pub fn i32_gt_s(&mut self) { self.code.push(op::OP_I32_GT_S); }
        pub fn i32_gt_u(&mut self) { self.code.push(op::OP_I32_GT_U); }
        pub fn i32_le_s(&mut self) { self.code.push(op::OP_I32_LE_S); }
        pub fn i32_le_u(&mut self) { self.code.push(op::OP_I32_LE_U); }
        pub fn i32_ge_s(&mut self) { self.code.push(op::OP_I32_GE_S); }
        pub fn i32_ge_u(&mut self) { self.code.push(op::OP_I32_GE_U); }
        
        // === Memory ===
        
        /// Load i32 from memory (address on stack)
        pub fn i32_load(&mut self, align: u32, offset: u32) {
            self.code.push(op::OP_I32_LOAD);
            self.write_leb128_u32(align);
            self.write_leb128_u32(offset);
        }
        
        /// Store i32 to memory (address and value on stack)
        pub fn i32_store(&mut self, align: u32, offset: u32) {
            self.code.push(op::OP_I32_STORE);
            self.write_leb128_u32(align);
            self.write_leb128_u32(offset);
        }
        
        // === Helpers ===
        
        fn label_depth(&self, label: Label) -> u32 {
            let target_depth = *self.label_depths.get(&label).unwrap();
            (self.label_stack.len() - 1 - target_depth) as u32
        }
        
        fn write_leb128_u32(&mut self, mut value: u32) {
            loop {
                let byte = (value & 0x7F) as u8;
                value >>= 7;
                if value == 0 {
                    self.code.push(byte);
                    break;
                } else {
                    self.code.push(byte | 0x80);
                }
            }
        }
        
        fn write_leb128_i32(&mut self, mut value: i32) {
            loop {
                let byte = (value & 0x7F) as u8;
                value >>= 7;
                let done = (value == 0 && byte & 0x40 == 0) 
                        || (value == -1 && byte & 0x40 != 0);
                if done {
                    self.code.push(byte);
                    break;
                } else {
                    self.code.push(byte | 0x80);
                }
            }
        }
    }
    
    impl Default for WasmBuilder {
        fn default() -> Self {
            Self::new()
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_operations() {
        let page = Page::of(0x8000_1234);
        assert_eq!(page.base_addr(), 0x8000_1000);
        assert!(page.contains(0x8000_1000));
        assert!(page.contains(0x8000_1FFF));
        assert!(!page.contains(0x8000_2000));
    }

    #[test]
    fn test_branch_condition() {
        let regs: [u32; 32] = {
            let mut r = [0u32; 32];
            r[1] = 10;
            r[2] = 20;
            r[3] = 10;
            r
        };

        let eq = BranchCondition::Eq { rs1: 1, rs2: 3 };
        assert!(eq.evaluate(&regs)); // 10 == 10

        let ne = BranchCondition::Ne { rs1: 1, rs2: 2 };
        assert!(ne.evaluate(&regs)); // 10 != 20

        let lt = BranchCondition::Lt { rs1: 1, rs2: 2 };
        assert!(lt.evaluate(&regs)); // 10 < 20
    }

    #[test]
    fn test_scc_simple() {
        // Simple graph: A -> B -> C
        let mut graph = CfgGraph::new();
        graph.insert(1, [2].into_iter().collect());
        graph.insert(2, [3].into_iter().collect());
        graph.insert(3, HashSet::new());

        let sccs = find_sccs(&graph);
        // Each node is its own SCC (no cycles)
        assert_eq!(sccs.len(), 3);
    }

    #[test]
    fn test_scc_loop() {
        // Graph with loop: A -> B -> C -> A
        let mut graph = CfgGraph::new();
        graph.insert(1, [2].into_iter().collect());
        graph.insert(2, [3].into_iter().collect());
        graph.insert(3, [1].into_iter().collect());

        let sccs = find_sccs(&graph);
        // All nodes in one SCC
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].len(), 3);
    }

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
