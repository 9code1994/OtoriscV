# Wonderhoy! 🌟 OtoriscV

<p align="center">
 <img src="images/emu_otori.png"/>
</p>

**OtoriscV** is a high-performance RISC-V emulator written in Rust, designed from the ground up with the goal of running full Linux environments in the browser. Inspired by the architecture of `jor1k` and `v86`, this project was "vibe coded" into existence from someone who doesn't know much about low-level programming so take this project as a grain of salt, but the git commit history is a good place to start learning about riscv emulators.

## ✨ Features

-   **RV32IMA** Support. (RV64GC WIP)
-   **Linux Bootable**: Runs modern kernels (v6.6+).
-   **VirtIO-9P Filesystem**: Mount host directories directly in the guest when running as cli.
-   **Optimized Performance**: Features XOR-based TLB caching, direct memory access paths, batched timer updates, basic block JIT, and an experimental Page-based JIT v2 with CFG optimization. (Inspired by Jor1k and v86)

---

## 🛠 Building and Running

### 1. Requirements
Ensure you have the Rust toolchain and `wasm-pack` installed.

### 2. Native CLI (Linux/macOS/Windows)
To run the emulator natively:
```bash
cargo build --release
```

### 3. WebAssembly (Browser/Node)
To build for the web:
```bash
wasm-pack build --target web --out-dir www/pkg
```

To build for Node.js (benchmarks):
```bash
wasm-pack build --target nodejs --out-dir node_pkg
```

---

## 🐧 Usage

OtoriscV looks for system images in the `images/` directory.

### Running Linux
```bash
./target/release/otoriscv images/Image-minimal --initrd images/rootfs_tcc.cpio --ram 64
```

Note: tcc can't compile with programs using lib C because of 128-bit instructions needed for linking even after adding stubs or patching the tcc rv32 fork.

### Useful Flags
-   `--ram <MB>`: Set the guest RAM size (default: 64MB).
-   `--benchmark`: Boots Linux and measures performance until a shell prompt is detected.
-   `--jit-v2`: Enables the experimental JIT v2 (Page-based JIT with CFG optimization) but slower than the default JIT when booting Linux to shell.
-   `--fs <path>`: Mount a local directory via VirtIO-9P (not tested yet).

---

## 📊 Benchmarking

To run the WASM performance benchmark in Node.js:
```bash
node tests/benchmark_wasm.js
```
This will boot a minimal Linux kernel and report the MIPS (Millions of Instructions Per Second) achieved by the WASM build.

On CLI you can use `--benchmark` to measure performance until a shell prompt is detected:
```bash
./target/release/otoriscv images/Image-minimal --initrd images/rootfs_tcc.cpio --benchmark
```

---

## 🏗 System Components

### Linux Kernel
Check `build-linux/` for example kernel configuration fragments. You can use these to build a minimal RISC-V kernel compatible with the emulator.

### Minimal Shell (init_minishell)
To compile the extremely minimal static shell found in `tests/`:
```bash
cd tests/minishell
riscv32-unknown-linux-gnu-gcc -static -o init init_minishell.c
# Then wrap it into a cpio archive:
echo init | cpio -H newc -o > minishell.cpio
```

### RootFS
We are planning to add Buildroot scripts in `buildroot-config/` to automate the creation of the kernel and root filesystem. For now, you can use the rootfs_tcc.cpio in `images/`.

---

## 📜 Documentation
For a deeper dive into the technical internals, check the `docs/` folder:
-   `uart_debugging_journey.md`: Reflections when couldn't boot Linux at first.
-   `jor1k_optimization_analysis.md`: Comparison with the jor1k emulator.
-   `performance_upgrade_plan.md`: The plan before implementing optimizations.
-   `jit_v2_debugging_journey.md`: Reflections on the JIT v2 implementation.
-   `rv64gc_upgrade_plan.md`: The plan for RV64GC support.

---

**Wonderhoy!** Happy emulating. 💫
