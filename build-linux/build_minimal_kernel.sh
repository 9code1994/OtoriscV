#!/bin/bash
# Build ultra-minimal Linux kernel for RISC-V 32-bit emulator
# Produces raw Image file (not EFI) suitable for direct boot

set -e

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LINUX_DIR="${PROJECT_ROOT}/build-linux/linux-6.6.70"
FRAGMENT_FILE="${PROJECT_ROOT}/build-linux/kernel_minimal_fragment.config"
OUTPUT_DIR="${PROJECT_ROOT}/images"
TOOLCHAIN_PREFIX="${PROJECT_ROOT}/opt/cross/riscv32-unknown-linux-gnu/bin/riscv32-unknown-linux-gnu-"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

# Check toolchain
if [ ! -x "${TOOLCHAIN_PREFIX}gcc" ]; then
    error "RISC-V toolchain not found at ${TOOLCHAIN_PREFIX}gcc"
fi

info "Using toolchain: $(${TOOLCHAIN_PREFIX}gcc --version | head -1)"

# Check Linux source
if [ ! -d "${LINUX_DIR}" ]; then
    error "Linux source not found at ${LINUX_DIR}"
fi

cd "${LINUX_DIR}"

# Clean previous build (optional, comment out to keep .o files)
if [ "$1" = "clean" ]; then
    info "Cleaning previous build..."
    make ARCH=riscv CROSS_COMPILE="${TOOLCHAIN_PREFIX}" mrproper
fi

# Apply minimal config
info "Starting with allnoconfig base..."
make ARCH=riscv CROSS_COMPILE="${TOOLCHAIN_PREFIX}" allnoconfig

info "Merging minimal fragment..."
scripts/kconfig/merge_config.sh -m .config "${FRAGMENT_FILE}"

# Update config with defaults for any missing options
info "Running olddefconfig..."
make ARCH=riscv CROSS_COMPILE="${TOOLCHAIN_PREFIX}" olddefconfig

# Show what changed
info "Key config settings:"
grep -E "^CONFIG_SMP=|^CONFIG_FPU=|^CONFIG_EXT4|^CONFIG_BTRFS|^CONFIG_CRYPTO=|^CONFIG_NET=|^CONFIG_DEBUG_KERNEL=|^CONFIG_SECURITY=|^CONFIG_EFI=" .config || true
grep -E "^# CONFIG_SMP|^# CONFIG_FPU|^# CONFIG_EXT4|^# CONFIG_BTRFS|^# CONFIG_CRYPTO |^# CONFIG_NET |^# CONFIG_DEBUG_KERNEL|^# CONFIG_SECURITY |^# CONFIG_EFI " .config || true

# Build kernel - use all cores
NPROC=$(nproc)
info "Building kernel with ${NPROC} cores..."

make ARCH=riscv CROSS_COMPILE="${TOOLCHAIN_PREFIX}" -j${NPROC} Image

# Check result
if [ ! -f arch/riscv/boot/Image ]; then
    error "Kernel Image not found after build!"
fi

# Copy to images directory
mkdir -p "${OUTPUT_DIR}"
cp arch/riscv/boot/Image "${OUTPUT_DIR}/Image-minimal"

# Create compressed versions
info "Creating compressed versions..."
gzip -9 -k -f "${OUTPUT_DIR}/Image-minimal"
zstd -19 -f "${OUTPUT_DIR}/Image-minimal" -o "${OUTPUT_DIR}/Image-minimal.zst"

# Report sizes
info "Build complete! Image sizes:"
ls -lh "${OUTPUT_DIR}/Image-minimal"*

info ""
info "To test boot:"
info "  cargo run --release -- images/Image-minimal --initrd images/rootfs.cpio --ram 64"
