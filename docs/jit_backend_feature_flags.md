# JIT Backend Feature Flags Implementation Plan

This document outlines the plan to add feature-flagged JIT compilation backends to otoriscv, with support for **Cranelift** and **dynasm-rs**.

## Overview

| Backend | Approach | Target Output | WASM Support | Complexity |
|---------|----------|---------------|--------------|------------|
| **Interpreter** (default) | Current implementation | N/A | ✅ | Low |
| **dynasm-rs** | Direct machine code emission | Host native (x86_64, aarch64, riscv) | ❌ | Medium |
| **Cranelift** | IR → optimized machine code | Host native (many targets) | ❌ | High |
| **WASM JIT** (future) | Generate WASM at runtime | WASM modules | ✅ | Very High |

> [!NOTE]
> dynasm-rs and Cranelift generate native code, so they only work for the native CLI target, not WASM. For WASM, the interpreter remains the only option (or a future WASM JIT like v86).

---

## Feature Flags Design

### Cargo.toml Changes

```toml
[features]
default = ["console_error_panic_hook"]
jit-dynasm = ["dynasm", "dynasmrt"]
jit-cranelift = [
    "cranelift-codegen", 
    "cranelift-frontend", 
    "cranelift-module",
    "cranelift-jit"
]

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
# dynasm-rs (generates native machine code)
dynasm = { version = "4.0", optional = true }
dynasmrt = { version = "4.0", optional = true }

# Cranelift (IR-based codegen)
cranelift-codegen = { version = "0.127", optional = true }
cranelift-frontend = { version = "0.127", optional = true }
cranelift-module = { version = "0.127", optional = true }
cranelift-jit = { version = "0.127", optional = true }
```

### Module Structure

```
src/
├── jit/
│   ├── mod.rs              # JIT trait definition, feature-gated re-exports
│   ├── basic_block.rs      # Basic block detection & analysis
│   ├── dynasm_backend.rs   # dynasm-rs implementation (cfg(feature = "jit-dynasm"))
│   └── cranelift_backend.rs # Cranelift implementation (cfg(feature = "jit-cranelift"))
└── cpu/rv32/
    └── mod.rs              # Integration: call JIT for hot code
```

---

## JIT Trait Interface

```rust
// src/jit/mod.rs

/// A compiled basic block that can be executed
pub trait CompiledBlock: Send + Sync {
    /// Execute the block, returning the next PC (or exit reason)
    /// 
    /// Takes mutable references to CPU state for register/memory access
    unsafe fn execute(&self, cpu_state: &mut CpuState) -> BlockExit;
}

/// Exit reason from a compiled block
pub enum BlockExit {
    /// Continue to specified PC
    Continue(u32),
    /// Trap occurred (ecall, exception, etc.)
    Trap(Trap),
    /// Need to fall back to interpreter (e.g., CSR access, privileged inst)
    Fallback,
}

/// JIT compiler backend trait
pub trait JitBackend {
    type CompiledBlock: CompiledBlock;
    
    /// Compile a basic block starting at the given physical address
    fn compile(
        &mut self,
        phys_addr: u32,
        memory: &[u8],
        max_instructions: usize,
    ) -> Result<Self::CompiledBlock, JitError>;
    
    /// Invalidate compiled code for a physical page
    fn invalidate_page(&mut self, page_addr: u32);
    
    /// Clear all compiled code
    fn clear_all(&mut self);
}
```

---

## dynasm-rs Backend

### How It Works

dynasm-rs provides a macro-based assembler that emits machine code at compile time (the macro) with runtime data. For JIT, we generate host-native code that:

1. Loads emulated registers from a CPU state struct pointer
2. Executes translated RISC-V instructions as native host ops
3. Stores modified registers back and returns next PC

### Key API Usage

```rust
use dynasm::dynasm;
use dynasmrt::{DynasmApi, DynasmLabelApi, Assembler, ExecutableBuffer};

#[cfg(target_arch = "x86_64")]
fn compile_add(ops: &mut Assembler<x64::X64Relocation>, rd: u8, rs1: u8, rs2: u8) {
    // Assuming cpu_state pointer in RDI, regs at offset 0
    let rs1_off = (rs1 as i32) * 4;
    let rs2_off = (rs2 as i32) * 4;
    let rd_off = (rd as i32) * 4;
    
    dynasm!(ops
        ; mov eax, [rdi + rs1_off]   // eax = regs[rs1]
        ; add eax, [rdi + rs2_off]   // eax += regs[rs2]
        ; mov [rdi + rd_off], eax    // regs[rd] = eax
    );
}
```

### Supported Host Architectures

- `x86_64` - Full support with extensive instruction set
- `aarch64` - Good support
- `riscv64` - Native RISC-V (but we're *emulating* RISC-V, so less useful)
- `x86` - 32-bit x86

### dynasm-rs RISC-V Note

> [!IMPORTANT]
> dynasm-rs has RISC-V target support for *generating* RISC-V code, but for our JIT we need to *emit host code* (x86_64/aarch64) that implements RISC-V semantics. We'll use the x64 or aarch64 modules.

---

## Cranelift Backend

### How It Works

Cranelift is an IR-based compiler. We:

1. Translate RISC-V instructions to Cranelift IR using `FunctionBuilder`
2. Cranelift optimizes and generates native code
3. Get executable function pointer via `cranelift-jit`

### Key API Usage

```rust
use cranelift_codegen::ir::{types::*, AbiParam, Function, InstBuilder, Signature};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};

fn compile_basic_block(
    module: &mut JITModule,
    instructions: &[(u32, DecodedInst)], // (addr, decoded)
) -> Result<*const u8, Error> {
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params.push(AbiParam::new(I64)); // cpu_state pointer
    sig.returns.push(AbiParam::new(I32)); // next PC or trap code
    
    let func_id = module.declare_function("bb", Linkage::Local, &sig)?;
    
    let mut ctx = module.make_context();
    ctx.func.signature = sig;
    
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
    
    // Declare variables for 32 registers
    let regs: Vec<Variable> = (0..32)
        .map(|i| builder.declare_var(I32))
        .collect();
    
    let entry = builder.create_block();
    builder.switch_to_block(entry);
    builder.seal_block(entry);
    
    // Load CPU state pointer parameter
    let cpu_state = builder.block_params(entry)[0];
    
    // For each RISC-V instruction, emit Cranelift IR
    for (addr, inst) in instructions {
        emit_instruction(&mut builder, &regs, cpu_state, inst);
    }
    
    builder.finalize();
    module.define_function(func_id, &mut ctx)?;
    module.finalize_definitions()?;
    
    Ok(module.get_finalized_function(func_id))
}

fn emit_instruction(
    builder: &mut FunctionBuilder,
    regs: &[Variable],
    cpu_state: Value,
    inst: &DecodedInst,
) {
    match inst.opcode {
        OP_OP => {
            let rs1_val = builder.use_var(regs[inst.rs1 as usize]);
            let rs2_val = builder.use_var(regs[inst.rs2 as usize]);
            
            let result = match inst.funct3 {
                FUNCT3_ADD_SUB if inst.funct7 == 0 => {
                    builder.ins().iadd(rs1_val, rs2_val)
                }
                FUNCT3_ADD_SUB => {
                    builder.ins().isub(rs1_val, rs2_val)
                }
                // ... other operations
                _ => todo!(),
            };
            
            if inst.rd != 0 {
                builder.def_var(regs[inst.rd as usize], result);
            }
        }
        // ... other opcodes
        _ => {}
    }
}
```

---

## Integration with CPU

### Hot Code Detection

```rust
// src/cpu/rv32/mod.rs

pub struct Cpu {
    // Existing fields...
    
    /// Execution count per physical page (for hotness tracking)
    #[cfg(any(feature = "jit-dynasm", feature = "jit-cranelift"))]
    page_heat: HashMap<u32, u32>,
    
    /// Compiled code cache
    #[cfg(any(feature = "jit-dynasm", feature = "jit-cranelift"))]
    code_cache: CodeCache,
}

impl Cpu {
    pub fn step(&mut self, bus: &mut impl Bus) -> Result<(), Trap> {
        let paddr = self.translate_pc(bus)?;
        
        #[cfg(any(feature = "jit-dynasm", feature = "jit-cranelift"))]
        {
            let page = paddr & !0xFFF;
            if let Some(block) = self.code_cache.get(paddr) {
                // Execute JIT-compiled code
                match unsafe { block.execute(&mut self.state) } {
                    BlockExit::Continue(next_pc) => {
                        self.pc = next_pc;
                        return Ok(());
                    }
                    BlockExit::Trap(trap) => return Err(trap),
                    BlockExit::Fallback => {} // Fall through to interpreter
                }
            } else {
                // Track hotness
                let count = self.page_heat.entry(page).or_insert(0);
                *count += 1;
                if *count >= JIT_THRESHOLD {
                    self.compile_page(page, bus);
                }
            }
        }
        
        // Interpreter path
        self.step_interpreter(bus)
    }
}
```

---

## Platform Considerations

### Native Targets

| Feature | x86_64 | aarch64 | riscv64 |
|---------|--------|---------|---------|
| dynasm-rs | ✅ | ✅ | ✅ (less useful) |
| Cranelift | ✅ | ✅ | ⚠️ (experimental) |

### WASM Target

Neither dynasm-rs nor Cranelift work in WASM. For WASM:
- Continue using interpreter
- Future: WASM JIT (generate WASM modules at runtime, like v86)

```rust
#[cfg(target_arch = "wasm32")]
compile_error!("JIT features are not supported on WASM target");
```

---

## Implementation Phases

### Phase 1: Infrastructure (P0)

1. Add feature flags to `Cargo.toml`
2. Create `src/jit/mod.rs` with trait definitions
3. Add basic block detection (`src/jit/basic_block.rs`)
4. Add code cache structure
5. Integrate with CPU step function (with fallback to interpreter)

### Phase 2: dynasm-rs Backend (P1)

1. Implement `DynasmBackend` for x86_64
2. Support RV32I base instructions (ALU, Load, Store)
3. Support branches and jumps
4. Handle traps (ecall → return to interpreter)
5. Optional: aarch64 support

### Phase 3: Cranelift Backend (P2)

1. Implement `CraneliftBackend`
2. Translate RV32I to Cranelift IR
3. Optimize: let Cranelift handle register allocation, CSE, etc.
4. Handle M extension (multiply/divide)

### Phase 4: Optimizations (P3)

1. Instruction combining (e.g., load-add → single host load+add)
2. Block linking (chain basic blocks without returning to dispatcher)
3. Trace-based JIT (compile hot traces, not just blocks)

---

## Build & Test

### Building with Feature Flags

```bash
# Interpreter only (default, works on WASM)
cargo build --release

# With dynasm-rs JIT (native only)
cargo build --release --features jit-dynasm

# With Cranelift JIT (native only)
cargo build --release --features jit-cranelift

# Both JITs (for benchmarking comparison)
cargo build --release --features "jit-dynasm,jit-cranelift"
```

### Verification Plan

Since this adds runtime code generation which is inherently unsafe and platform-specific:

1. **Existing tests must still pass** (interpreter correctness baseline):
   ```bash
   cargo test
   ```

2. **JIT-specific unit tests** (to be added in `src/jit/tests.rs`):
   - Test basic block detection
   - Test code cache invalidation
   - Test individual instruction translation

3. **Linux boot test** (integration):
   ```bash
   # Compare interpreter vs JIT boot time
   cargo run --release -- --kernel build-linux/kernel.bin --ram 64
   cargo run --release --features jit-dynasm -- --kernel build-linux/kernel.bin --ram 64
   ```

4. **RISC-V compliance tests** (correctness):
   ```bash
   cd tools/compliance
   make DEVICE=otoriscv run
   ```

5. **Manual benchmark**: Time to Linux shell prompt with/without JIT.

---

## References

- [dynasm-rs docs](https://censoredusername.github.io/dynasm-rs/)
- [dynasmrt crate](https://censoredusername.github.io/dynasm-rs/dynasmrt/index.html)
- [dynasm-rs RISC-V reference](https://censoredusername.github.io/dynasm-rs/language/langref_riscv.html)
- [Cranelift codegen](https://docs.rs/cranelift-codegen/latest/cranelift_codegen/)
- [Cranelift frontend](https://docs.rs/cranelift-frontend/latest/cranelift_frontend/)
- [v86 how-it-works](../../references/v86/docs/how-it-works.md) - WASM JIT reference
