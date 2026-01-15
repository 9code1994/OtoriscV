const fs = require('fs');
const path = require('path');
const zlib = require('zlib');
const { Emulator, decompress_zstd } = require('../node_pkg/otoriscv');

async function runBenchmark() {
    console.log("Starting WASM Node.js Benchmark...");

    // 1. Load assets
    const kernelPath = path.join(__dirname, '../images/Image-minimal.zst');
    const initrdPath = path.join(__dirname, '../images/minishell.cpio');

    console.log(`Loading kernel: ${kernelPath}`);
    const kernelCompressed = fs.readFileSync(kernelPath);
    // const kernel = zlib.gunzipSync(kernelGzip);

    // Decompress using WASM zstd
    console.log("Decompressing kernel with zstd...");
    const kernel = decompress_zstd(kernelCompressed);

    console.log(`Loading initrd: ${initrdPath}`);
    const initrd = fs.readFileSync(initrdPath);

    // 2. Initialize Emulator
    // RAM size: 64MB
    const emu = new Emulator(64);

    // Command line for minimal boot
    const cmdline = "lpj=10000 console=ttyS0 earlycon rdinit=/sbin/init";

    console.log("Setting up Linux boot...");
    emu.setup_linux_with_initrd(kernel, initrd, cmdline);
    // emu.enable_jit_v2(true);

    const startTime = Date.now();
    let totalInstructions = 0;
    let outputBuffer = "";
    const promptRegex = /\n(#|\$|~ \$|~#) $/;

    console.log("Emulation started. Waiting for shell prompt...");

    const maxInstructions = 1_000_000_000; // 1B instructions timeout
    const batchSize = 100000;

    while (totalInstructions < maxInstructions) {
        const cycles = emu.run(batchSize);
        totalInstructions += cycles;

        // Get output
        const output = emu.get_uart_output();
        if (output.length > 0) {
            const text = Buffer.from(output).toString();
            process.stdout.write(text);
            outputBuffer += text;

            // Keep buffer reasonable
            if (outputBuffer.length > 1000) {
                outputBuffer = outputBuffer.slice(-1000);
            }

            if (promptRegex.test(outputBuffer)) {
                console.log("\n\nShell prompt detected!");
                break;
            }
        }

        if (cycles === 0) {
            console.log("\nEmulator halted.");
            break;
        }
    }

    const endTime = Date.now();
    const durationSec = (endTime - startTime) / 1000;
    const mips = (totalInstructions / 1000000) / durationSec;

    console.log("-----------------------------------------");
    console.log(`Benchmark Finished`);
    console.log(`Total Instructions: ${totalInstructions.toLocaleString()}`);
    console.log(`Wall Time: ${durationSec.toFixed(3)}s`);
    console.log(`Performance: ${mips.toFixed(2)} MIPS`);
    console.log("-----------------------------------------");

    if (totalInstructions >= maxInstructions) {
        console.error("Timeout: Shell prompt not reached.");
        process.exit(1);
    }
}

runBenchmark().catch(err => {
    console.error(err);
    process.exit(1);
});
