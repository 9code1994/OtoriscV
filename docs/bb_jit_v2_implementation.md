# BB-JIT V2: v86-Style Advanced JIT Implementation

## Overview

This document outlines the implementation of an advanced JIT compilation system inspired by v86's architecture. The key innovations from v86 include:

1. **WebAssembly Function Table** - Direct dispatch to compiled code
2. **Control Flow Graph (CFG) Structuring** - Kosaraju's SCC algorithm for loops
3. **WasmBuilder** - Efficient bytecode generation
4. **Hotness-Based Compilation** - Only compile frequently executed code
5. **Multi-Entry Point Handling** - Dispatcher br_table for complex control flow

## Current State (bb_jit.rs v1)

Our current implementation:
- Simple HashMap-based block cache
- Linear instruction execution within blocks  
- Generation counter for invalidation
- No CFG optimization
- No WebAssembly output

Performance:
- ~3M IPS with block caching
- Good hit rate but limited optimization potential

## v86 Architecture Analysis

### JIT Table Structure

v86 uses a WebAssembly function table to store compiled modules:

```rust
// v86 constants
pub const WASM_TABLE_SIZE: u32 = 900;   // Max compiled modules
pub const JIT_THRESHOLD: u32 = 200_000;  // Hotness before compilation

// Per-page compilation info
struct PageInfo {
    wasm_table_index: WasmTableIndex,
    entry_points: Vec<(u16, u16)>,     // (page_offset, state_index)
    state_flags: CachedStateFlags,
}

// Cached code pointer stored in TLB
struct CachedCode {
    wasm_table_index: WasmTableIndex,
    initial_state: u16,
}
```

### Control Flow Graph Structuring

v86 converts basic blocks into structured WebAssembly control flow:

```rust
pub enum WasmStructure {
    BasicBlock(u32),           // Single basic block
    Dispatcher(Vec<u32>),      // Entry point dispatcher (br_table)
    Loop(Vec<WasmStructure>),  // Wasm loop construct
    Block(Vec<WasmStructure>), // Wasm block construct
}

// Pipeline:
// 1. make_graph() - Build CFG from basic blocks
// 2. scc() - Find strongly connected components (loops)
// 3. loopify() - Convert SCCs to WasmStructure::Loop
// 4. blockify() - Add Block wrappers for forward jumps
```

### WasmBuilder Key Features

```rust
pub struct WasmBuilder {
    output: Vec<u8>,              // Final wasm module
    instruction_body: Vec<u8>,    // Current function body
    
    // Label management for structured control flow
    label_stack: Vec<Label>,
    label_to_depth: HashMap<Label, usize>,
    
    // Local variable management  
    free_locals_i32: Vec<WasmLocal>,
    local_count: u8,
}

impl WasmBuilder {
    // Control flow
    fn loop_void(&mut self) -> Label;
    fn block_void(&mut self) -> Label;
    fn br(&mut self, label: Label);
    fn brtable(&mut self, default: Label, cases: &[Label]);
    
    // Memory access
    fn load_aligned_i32(&mut self, offset: u32);
    fn store_aligned_i32(&mut self, offset: u32);
    
    // Arithmetic
    fn add_i32(&mut self);
    fn and_i32(&mut self);
    fn call_fn(&mut self, name: &str);
}
```

---

## BB-JIT V2 Design

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                     BB-JIT V2 Pipeline                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  1. Hotness Tracking (per-page)                                 │
│     ┌──────────────┐     threshold    ┌─────────────────┐       │
│     │ Interpreter  │ ───────────────> │ Trigger Compile │       │
│     │ (count heat) │    >= 100k       │                 │       │
│     └──────────────┘                  └────────┬────────┘       │
│                                                │                 │
│  2. Basic Block Discovery                      ▼                 │
│     ┌─────────────────────────────────────────────────┐         │
│     │ discover_basic_blocks()                         │         │
│     │ - Follow control flow from entry points         │         │
│     │ - Mark BRANCH, JAL, JALR, SYSTEM as terminators │         │
│     │ - Build BasicBlock list with edges              │         │
│     └─────────────────────────────────────────────────┘         │
│                                                │                 │
│  3. CFG Analysis                               ▼                 │
│     ┌─────────────────────────────────────────────────┐         │
│     │ build_cfg() → find_sccs() → structure()        │         │
│     │ - Kosaraju's algorithm for loop detection      │         │
│     │ - Convert to ControlFlowStructure              │         │
│     └─────────────────────────────────────────────────┘         │
│                                                │                 │
│  4. Code Generation                            ▼                 │
│     ┌─────────────────────────────────────────────────┐         │
│     │ Native Target:                                  │         │
│     │ - Generate closure-based executors              │         │
│     │ - Registers in Rust variables (compiler opts)   │         │
│     │                                                 │         │
│     │ WASM Target:                                    │         │
│     │ - WasmBuilder generates .wasm modules           │         │
│     │ - br_table dispatcher for entry points          │         │
│     │ - loop/block for structured control flow        │         │
│     └─────────────────────────────────────────────────┘         │
│                                                │                 │
│  5. Cache & Execute                            ▼                 │
│     ┌─────────────────────────────────────────────────┐         │
│     │ JIT Table stores compiled regions               │         │
│     │ Direct dispatch via function pointer/wasm call  │         │
│     └─────────────────────────────────────────────────┘         │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Core Data Structures

```rust
/// Physical page identifier (upper 20 bits of address)
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub struct Page(u32);

impl Page {
    #[inline(always)]
    pub fn of(addr: u32) -> Self { Page(addr >> 12) }
    pub fn base_addr(self) -> u32 { self.0 << 12 }
}

/// Basic block with control flow edges
pub struct BasicBlock {
    pub addr: u32,                       // Physical start address
    pub end_addr: u32,                   // Physical end address (exclusive)
    pub instructions: Vec<CachedInst>,   // Decoded instructions
    pub ty: BasicBlockType,              // Terminator type
    pub is_entry_point: bool,            // Can be jumped to from outside
}

/// Basic block terminator types
pub enum BasicBlockType {
    /// Fallthrough to next block
    Fallthrough { next: Option<u32> },
    /// Unconditional jump (JAL with rd=x0, tail calls)
    Jump { target: Option<u32> },
    /// Conditional branch
    Branch {
        taken: Option<u32>,
        not_taken: Option<u32>,
        condition: BranchCondition,
    },
    /// Indirect jump (JALR, computed gotos)
    IndirectJump,
    /// System call or trap-generating instruction
    Exit,
}

/// Branch condition for conditional compilation
pub enum BranchCondition {
    Eq(u8, u8),   // BEQ rs1, rs2
    Ne(u8, u8),   // BNE rs1, rs2  
    Lt(u8, u8),   // BLT rs1, rs2
    Ge(u8, u8),   // BGE rs1, rs2
    Ltu(u8, u8),  // BLTU rs1, rs2
    Geu(u8, u8),  // BGEU rs1, rs2
}

/// Structured control flow for code generation
pub enum ControlFlowStructure {
    /// Single basic block
    Block(u32),
    /// Entry point dispatcher (multiple entries into region)
    Dispatcher(Vec<u32>),
    /// Loop (back-edges in CFG)
    Loop(Vec<ControlFlowStructure>),
    /// Block for forward jumps
    Forward(Vec<ControlFlowStructure>),
}

/// Compiled region (one or more pages)
pub struct CompiledRegion {
    /// Pages covered by this region
    pub pages: Vec<Page>,
    /// Entry points: (page_offset, state_index)
    pub entry_points: Vec<(u16, u16)>,
    /// Executor function or wasm table index
    pub executor: RegionExecutor,
    /// Generation for invalidation
    pub generation: u32,
}

/// Region executor - platform-specific
pub enum RegionExecutor {
    /// Native: closure that executes the region
    #[cfg(not(target_arch = "wasm32"))]
    Native(Box<dyn Fn(&mut Cpu, &mut dyn Bus) -> BlockResult>),
    /// WASM: index into function table
    #[cfg(target_arch = "wasm32")]
    WasmTable { index: u16, entry_states: Vec<u16> },
}
```

### Hotness Tracking

```rust
/// Per-page execution statistics
pub struct PageStats {
    /// Execution count (increases by ~100 per basic block execution)
    pub hotness: u32,
    /// Known entry points into this page
    pub entry_points: HashSet<u16>,  // Page offsets
}

/// JIT state
pub struct JitState {
    /// Page statistics for hotness tracking
    page_stats: HashMap<Page, PageStats>,
    /// Compiled regions
    regions: HashMap<Page, CompiledRegion>,
    /// Generation counter
    generation: u32,
    /// Compilation threshold
    threshold: u32,
}

impl JitState {
    pub const DEFAULT_THRESHOLD: u32 = 100_000;
    
    /// Record execution and maybe trigger compilation
    #[inline]
    pub fn record_execution(&mut self, paddr: u32, heat: u32) -> Option<Page> {
        let page = Page::of(paddr);
        let offset = (paddr & 0xFFF) as u16;
        
        let stats = self.page_stats.entry(page).or_default();
        stats.entry_points.insert(offset);
        stats.hotness += heat;
        
        if stats.hotness >= self.threshold {
            stats.hotness = 0;
            Some(page)  // Signal: compile this page
        } else {
            None
        }
    }
}
```

### CFG Construction

```rust
type Graph = HashMap<u32, HashSet<u32>>;  // addr -> successors

/// Build CFG from list of basic blocks
pub fn build_cfg(blocks: &[BasicBlock]) -> Graph {
    let mut graph = Graph::new();
    
    for block in blocks {
        let mut successors = HashSet::new();
        
        match &block.ty {
            BasicBlockType::Fallthrough { next: Some(addr) } => {
                successors.insert(*addr);
            }
            BasicBlockType::Jump { target: Some(addr) } => {
                successors.insert(*addr);
            }
            BasicBlockType::Branch { taken, not_taken, .. } => {
                if let Some(addr) = taken { successors.insert(*addr); }
                if let Some(addr) = not_taken { successors.insert(*addr); }
            }
            _ => {}
        }
        
        graph.insert(block.addr, successors);
    }
    
    graph
}

/// Find strongly connected components (Kosaraju's algorithm)
pub fn find_sccs(graph: &Graph) -> Vec<Vec<u32>> {
    // 1. DFS to get finish order
    let mut visited = HashSet::new();
    let mut finish_order = Vec::new();
    
    for &node in graph.keys() {
        if !visited.contains(&node) {
            dfs_finish(node, graph, &mut visited, &mut finish_order);
        }
    }
    
    // 2. Build reverse graph
    let rev_graph = reverse_graph(graph);
    
    // 3. DFS in reverse finish order on reverse graph
    let mut visited = HashSet::new();
    let mut sccs = Vec::new();
    
    for &node in finish_order.iter().rev() {
        if !visited.contains(&node) {
            let mut component = Vec::new();
            dfs_collect(node, &rev_graph, &mut visited, &mut component);
            sccs.push(component);
        }
    }
    
    sccs
}

/// Convert CFG to structured control flow
pub fn structure_cfg(graph: &Graph, entry_points: &[u32]) -> Vec<ControlFlowStructure> {
    let sccs = find_sccs(graph);
    let mut result = Vec::new();
    
    // Add dispatcher if multiple entry points
    if entry_points.len() > 1 {
        result.push(ControlFlowStructure::Dispatcher(entry_points.to_vec()));
    }
    
    for scc in sccs {
        if scc.len() == 1 {
            let addr = scc[0];
            // Self-loop?
            if graph.get(&addr).map_or(false, |s| s.contains(&addr)) {
                result.push(ControlFlowStructure::Loop(vec![
                    ControlFlowStructure::Block(addr)
                ]));
            } else {
                result.push(ControlFlowStructure::Block(addr));
            }
        } else {
            // Multi-block loop - recursively structure
            let sub_graph = subgraph(graph, &scc);
            let inner = structure_cfg(&sub_graph, &[scc[0]]);
            result.push(ControlFlowStructure::Loop(inner));
        }
    }
    
    result
}
```

### Code Generation (Native Target)

For native targets, we generate optimized Rust closures:

```rust
/// Generate native executor for a structured region
pub fn generate_native(
    structure: &[ControlFlowStructure],
    blocks: &HashMap<u32, BasicBlock>,
) -> Box<dyn Fn(&mut Cpu, &mut dyn Bus) -> BlockResult> {
    // Clone blocks into the closure
    let blocks = blocks.clone();
    let structure = structure.to_vec();
    
    Box::new(move |cpu: &mut Cpu, bus: &mut dyn Bus| {
        execute_structure(cpu, bus, &structure, &blocks)
    })
}

fn execute_structure(
    cpu: &mut Cpu,
    bus: &mut dyn Bus,
    structure: &[ControlFlowStructure],
    blocks: &HashMap<u32, BasicBlock>,
) -> BlockResult {
    for node in structure {
        match node {
            ControlFlowStructure::Block(addr) => {
                if let Some(block) = blocks.get(addr) {
                    match execute_basic_block(cpu, bus, block) {
                        BlockResult::Continue(next_pc) => {
                            cpu.pc = next_pc;
                        }
                        result => return result,
                    }
                }
            }
            ControlFlowStructure::Loop(inner) => {
                loop {
                    match execute_structure(cpu, bus, inner, blocks) {
                        BlockResult::Continue(pc) => {
                            // Check if we should continue looping
                            let page = Page::of(pc);
                            if !blocks.contains_key(&pc) {
                                return BlockResult::Continue(pc);
                            }
                        }
                        result => return result,
                    }
                }
            }
            ControlFlowStructure::Dispatcher(entries) => {
                // Find which entry point matches current PC
                let offset = (cpu.pc & 0xFFF) as u16;
                // ... dispatch logic
            }
            ControlFlowStructure::Forward(inner) => {
                match execute_structure(cpu, bus, inner, blocks) {
                    result => return result,
                }
            }
        }
    }
    
    BlockResult::Continue(cpu.pc)
}
```

### Code Generation (WASM Target)

For WebAssembly, we generate wasm modules:

```rust
#[cfg(target_arch = "wasm32")]
pub mod wasm_codegen {
    use super::*;
    
    /// WebAssembly opcodes
    mod op {
        pub const OP_BLOCK: u8 = 0x02;
        pub const OP_LOOP: u8 = 0x03;
        pub const OP_IF: u8 = 0x04;
        pub const OP_BR: u8 = 0x0C;
        pub const OP_BR_IF: u8 = 0x0D;
        pub const OP_BR_TABLE: u8 = 0x0E;
        pub const OP_RETURN: u8 = 0x0F;
        pub const OP_CALL: u8 = 0x10;
        pub const OP_LOCAL_GET: u8 = 0x20;
        pub const OP_LOCAL_SET: u8 = 0x21;
        pub const OP_I32_LOAD: u8 = 0x28;
        pub const OP_I32_STORE: u8 = 0x36;
        pub const OP_I32_CONST: u8 = 0x41;
        pub const OP_I32_ADD: u8 = 0x6A;
        pub const OP_I32_SUB: u8 = 0x6B;
        pub const OP_I32_AND: u8 = 0x71;
        pub const OP_I32_OR: u8 = 0x72;
        pub const OP_I32_XOR: u8 = 0x73;
        pub const OP_I32_SHL: u8 = 0x74;
        pub const OP_I32_SHR_S: u8 = 0x75;
        pub const OP_I32_SHR_U: u8 = 0x76;
    }
    
    /// WASM module builder
    pub struct WasmBuilder {
        output: Vec<u8>,
        code: Vec<u8>,
        label_stack: Vec<u32>,
        next_label: u32,
    }
    
    impl WasmBuilder {
        pub fn new() -> Self {
            let mut builder = WasmBuilder {
                output: Vec::with_capacity(4096),
                code: Vec::with_capacity(2048),
                label_stack: Vec::new(),
                next_label: 0,
            };
            builder.init_module();
            builder
        }
        
        fn init_module(&mut self) {
            // WASM magic number and version
            self.output.extend_from_slice(b"\0asm");
            self.output.extend_from_slice(&[1, 0, 0, 0]);
        }
        
        pub fn begin_loop(&mut self) -> u32 {
            let label = self.next_label;
            self.next_label += 1;
            self.label_stack.push(label);
            self.code.push(op::OP_LOOP);
            self.code.push(0x40); // void block type
            label
        }
        
        pub fn begin_block(&mut self) -> u32 {
            let label = self.next_label;
            self.next_label += 1;
            self.label_stack.push(label);
            self.code.push(op::OP_BLOCK);
            self.code.push(0x40);
            label
        }
        
        pub fn end(&mut self) {
            self.label_stack.pop();
            self.code.push(0x0B); // end
        }
        
        pub fn br(&mut self, label: u32) {
            let depth = self.label_depth(label);
            self.code.push(op::OP_BR);
            self.write_leb128(depth);
        }
        
        pub fn br_table(&mut self, labels: &[u32], default: u32) {
            self.code.push(op::OP_BR_TABLE);
            self.write_leb128(labels.len() as u32);
            for &label in labels {
                self.write_leb128(self.label_depth(label));
            }
            self.write_leb128(self.label_depth(default));
        }
        
        fn label_depth(&self, label: u32) -> u32 {
            let pos = self.label_stack.iter().rposition(|&l| l == label).unwrap();
            (self.label_stack.len() - 1 - pos) as u32
        }
        
        fn write_leb128(&mut self, mut value: u32) {
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
        
        // ... more builder methods
    }
}
```

---

## Implementation Plan

### Phase 1: Core Data Structures

**Files:** `src/cpu/rv32/bb_jit_v2.rs`

1. Define `Page`, `BasicBlock`, `BasicBlockType`
2. Define `ControlFlowStructure` enum
3. Define `CompiledRegion` and `JitState`
4. Implement hotness tracking

### Phase 2: Basic Block Discovery

**Files:** `src/cpu/rv32/bb_jit_v2.rs`

1. Implement `discover_basic_blocks(bus, entry_points) -> Vec<BasicBlock>`
2. Follow control flow from entry points
3. Handle page boundaries
4. Detect block terminators (BRANCH, JAL, JALR, SYSTEM)

### Phase 3: CFG Analysis

**Files:** `src/cpu/rv32/cfg.rs`

1. Implement `build_cfg(blocks) -> Graph`
2. Implement `find_sccs(graph) -> Vec<Vec<u32>>` (Kosaraju's algorithm)
3. Implement `structure_cfg(graph, entries) -> Vec<ControlFlowStructure>`
4. Handle multi-entry loops with Dispatcher

**Status: ✅ COMPLETED**

### Phase 4: Native Code Generation

**Files:** `src/cpu/rv32/bb_jit_v2.rs`

1. ✅ Implement `execute_basic_block()` - Execute single BB and return next PC
2. ✅ Implement `execute_structure()` - Recursive structured control flow execution
3. ✅ Handle loop continuation and exit conditions with `MAX_LOOP_ITERATIONS`
4. ✅ Integrate with existing `execute_cached()` for instruction execution

**Status: ✅ COMPLETED**

### Phase 5: WASM Code Generation (Optional)

**Files:** `src/cpu/rv32/bb_jit_v2.rs` (wasm_codegen module)

1. ✅ Implement basic `WasmBuilder` with label stack management
2. ✅ Control flow opcodes: block, loop, if/else, br, br_if, br_table
3. ✅ Arithmetic opcodes: add, sub, mul, div, and, or, xor, shifts
4. ✅ Comparison opcodes: eq, ne, lt, gt, le, ge (signed/unsigned)
5. ✅ LEB128 encoding for integers
6. Conditionally compiled for `wasm32` target only

**Status: ✅ COMPLETED (foundation)**

### Phase 6: Integration

**Files:** `src/system.rs`, `src/cpu/rv32/mod.rs`

1. ✅ Add `JitState` field to System struct alongside existing `BlockCache`
2. ✅ Add `use_jit_v2` toggle with `enable_jit_v2()` method
3. ✅ Implement `step_block_v2()` using JitState
4. ✅ Update cache invalidation to clear both v1 and v2 caches on FENCE.I/SFENCE.VMA
5. ✅ Fallback to v1 execution when region not yet compiled

**Status: ✅ COMPLETED**

---

## Usage

```rust
// Enable JIT v2 (default is off for backward compatibility)
system.enable_jit_v2(true);

// Run normally - JIT v2 will be used automatically
system.run(cycles);
```

---

## Expected Performance

| Metric | v1 (Current) | v2 (Target) |
|--------|--------------|-------------|
| Block hit rate | ~95% | ~98% |
| Avg block size | ~8 insts | ~15 insts |
| Loop execution | Exit/re-enter | Native loop |
| Dispatcher overhead | N/A | 1 br_table |
| IPS (native) | ~3M | ~5-6M |
| IPS (wasm) | ~2M | ~3-4M |

---

## Key Optimizations from v86

1. **Hotness-based compilation** - Don't compile cold code
2. **Multi-entry dispatchers** - Handle complex CFGs efficiently  
3. **SCC-based loop detection** - Find natural loops for optimization
4. **Register caching** - Keep frequently-used values in locals
5. **Structured control flow** - Efficient WebAssembly generation
6. **Generation-based invalidation** - Fast bulk cache clearing

---

## Testing Strategy

1. ✅ **Unit tests** for CFG algorithms (SCC, structuring) - 5 tests passing
2. **Integration tests** with known control flow patterns
3. **Benchmark** Linux boot time before/after
4. **Correctness tests** - compliance test suite

---

## References

- [v86 JIT implementation](../references/v86/src/rust/jit.rs)
- [v86 control flow analysis](../references/v86/src/rust/control_flow.rs)
- [v86 WASM builder](../references/v86/src/rust/wasmgen/wasm_builder.rs)
- [WebAssembly specification](https://webassembly.github.io/spec/)
