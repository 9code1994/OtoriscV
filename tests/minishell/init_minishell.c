// Mini shell with basic commands
// Compile: riscv32-...-gcc -static -nostdlib -o init init_minishell.c

// Initialize GP (global pointer) - required for accessing global variables
// when compiled with -nostdlib (no C runtime startup)
__asm__(
    ".section .text\n"
    ".global _start\n"
    "_start:\n"
    "    .option push\n"
    "    .option norelax\n"
    "    la gp, __global_pointer$\n"
    "    .option pop\n"
    "    j _start_c\n"
);

#define SYS_getcwd       17
#define SYS_mkdirat      34
#define SYS_openat       56
#define SYS_close        57
#define SYS_getdents64   61
#define SYS_read         63
#define SYS_write        64
#define SYS_exit         93
#define SYS_uname        160
#define SYS_mount        40
#define SYS_chdir        49
#define SYS_nanosleep    101
#define SYS_sched_yield  124

#define AT_FDCWD         -100
#define O_RDONLY         0
#define O_CREAT          0100
#define O_WRONLY         1

// Syscall wrappers
static long syscall1(long n, long a0) {
    register long _a0 __asm__("a0") = a0;
    register long _n  __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(_a0) : "r"(_n) : "memory");
    return _a0;
}

static long syscall2(long n, long a0, long a1) {
    register long _a0 __asm__("a0") = a0;
    register long _a1 __asm__("a1") = a1;
    register long _n  __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(_a0) : "r"(_a1), "r"(_n) : "memory");
    return _a0;
}

static long syscall3(long n, long a0, long a1, long a2) {
    register long _a0 __asm__("a0") = a0;
    register long _a1 __asm__("a1") = a1;
    register long _a2 __asm__("a2") = a2;
    register long _n  __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(_a0) : "r"(_a1), "r"(_a2), "r"(_n) : "memory");
    return _a0;
}

static long syscall4(long n, long a0, long a1, long a2, long a3) {
    register long _a0 __asm__("a0") = a0;
    register long _a1 __asm__("a1") = a1;
    register long _a2 __asm__("a2") = a2;
    register long _a3 __asm__("a3") = a3;
    register long _n  __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(_a0) : "r"(_a1), "r"(_a2), "r"(_a3), "r"(_n) : "memory");
    return _a0;
}

static long syscall5(long n, long a0, long a1, long a2, long a3, long a4) {
    register long _a0 __asm__("a0") = a0;
    register long _a1 __asm__("a1") = a1;
    register long _a2 __asm__("a2") = a2;
    register long _a3 __asm__("a3") = a3;
    register long _a4 __asm__("a4") = a4;
    register long _n  __asm__("a7") = n;
    __asm__ volatile("ecall" : "+r"(_a0) : "r"(_a1), "r"(_a2), "r"(_a3), "r"(_a4), "r"(_n) : "memory");
    return _a0;
}

// Basic I/O
static void put_char(char c) {
    syscall3(SYS_write, 1, (long)&c, 1);
}

static void print(const char *s) {
    while (*s) put_char(*s++);
}

// String comparison
static int streq(const char *a, const char *b) {
    while (*a && *b && *a == *b) { a++; b++; }
    return (*a == 0 && *b == 0);
}

static int starts_with(const char *s, const char *prefix) {
    while (*prefix) {
        if (*s++ != *prefix++) return 0;
    }
    return 1;
}

// Skip whitespace
static const char *skip_ws(const char *s) {
    while (*s == ' ' || *s == '\t') s++;
    return s;
}

// Directory entry
struct dirent64 {
    unsigned long long d_ino;
    long long d_off;
    unsigned short d_reclen;
    unsigned char d_type;
    char d_name[];
};

// uname structure (must match kernel's new_utsname - 6 fields!)
struct utsname {
    char sysname[65];
    char nodename[65];
    char release[65];
    char version[65];
    char machine[65];
    char domainname[65];  // Often forgotten but required!
};

// Command buffer
static char cmd[128];
static int cmd_len;

// Yield CPU to allow kernel to process interrupts
static void yield_cpu(void) {
    // Use sched_yield - simpler than nanosleep and doesn't need timespec struct
    syscall1(SYS_sched_yield, 0);
}

// Read a line
static void readline(void) {
    cmd_len = 0;
    char c;
    while (cmd_len < 126) {
        long n = syscall3(SYS_read, 0, (long)&c, 1);
        if (n <= 0) {
            // No data - yield to let kernel process interrupts
            yield_cpu();
            continue;
        }
        
        if (c == '\n' || c == '\r') {
            put_char('\n');
            cmd[cmd_len] = 0;
            return;
        } else if (c == 127 || c == 8) {
            if (cmd_len > 0) {
                cmd_len--;
                print("\b \b");
            }
        } else if (c >= 32 && c < 127) {
            cmd[cmd_len++] = c;
            put_char(c);
        }
    }
    cmd[cmd_len] = 0;
}

// Commands
static void do_help(void) {
    print("Commands:\n");
    print("  help       - this help\n");
    print("  ls [dir]   - list directory\n");
    print("  cd <dir>   - change directory\n");
    print("  pwd        - print directory\n");
    print("  cat <file> - show file\n");
    print("  touch <f>  - create file\n");
    print("  mkdir <d>  - create directory\n");
    print("  mount <src> <dst> <type>\n");
    print("  uname      - system info\n");
    print("  echo <txt> - print text\n");
}

static void do_ls(const char *path) {
    if (!path || !*path) path = ".";
    
    int fd = syscall4(SYS_openat, AT_FDCWD, (long)path, O_RDONLY, 0);
    if (fd < 0) {
        print("ls: error\n");
        return;
    }
    
    char buf[256];
    int n;
    while ((n = syscall3(SYS_getdents64, fd, (long)buf, 256)) > 0) {
        int pos = 0;
        while (pos < n) {
            struct dirent64 *d = (struct dirent64 *)(buf + pos);
            if (d->d_type == 4) print("d ");
            else if (d->d_type == 8) print("- ");
            else print("? ");
            print(d->d_name);
            put_char('\n');
            pos += d->d_reclen;
        }
    }
    syscall1(SYS_close, fd);
}

static void do_cd(const char *path) {
    if (!path || !*path) path = "/";
    if (syscall1(SYS_chdir, (long)path) < 0) {
        print("cd: error\n");
    }
}

static void do_pwd(void) {
    char buf[128];
    if (syscall2(SYS_getcwd, (long)buf, 128) > 0) {
        print(buf);
        put_char('\n');
    }
}

static void do_cat(const char *path) {
    if (!path || !*path) { print("cat: need file\n"); return; }
    
    int fd = syscall4(SYS_openat, AT_FDCWD, (long)path, O_RDONLY, 0);
    if (fd < 0) { print("cat: error\n"); return; }
    
    char buf[128];
    int n;
    while ((n = syscall3(SYS_read, fd, (long)buf, 128)) > 0) {
        syscall3(SYS_write, 1, (long)buf, n);
    }
    syscall1(SYS_close, fd);
}

static void do_touch(const char *path) {
    if (!path || !*path) { print("touch: need file\n"); return; }
    int fd = syscall4(SYS_openat, AT_FDCWD, (long)path, O_CREAT | O_WRONLY, 0644);
    if (fd >= 0) syscall1(SYS_close, fd);
    else print("touch: error\n");
}

static void do_mkdir(const char *path) {
    if (!path || !*path) { print("mkdir: need dir\n"); return; }
    if (syscall3(SYS_mkdirat, AT_FDCWD, (long)path, 0755) < 0) {
        print("mkdir: error\n");
    }
}

static void do_mount(const char *args) {
    // Parse: src dst type
    static char src[32], dst[32], type[16];
    int i = 0, j = 0;
    
    // Parse source
    args = skip_ws(args);
    while (*args && *args != ' ' && i < 31) src[i++] = *args++;
    src[i] = 0;
    
    // Parse dest
    args = skip_ws(args);
    i = 0;
    while (*args && *args != ' ' && i < 31) dst[i++] = *args++;
    dst[i] = 0;
    
    // Parse type
    args = skip_ws(args);
    i = 0;
    while (*args && *args != ' ' && i < 15) type[i++] = *args++;
    type[i] = 0;
    
    if (!*src || !*dst || !*type) {
        print("mount: <src> <dst> <type>\n");
        return;
    }
    
    if (syscall5(SYS_mount, (long)src, (long)dst, (long)type, 0, 0) == 0) {
        print("OK\n");
    } else {
        print("mount: error\n");
    }
}

static void do_uname(void) {
    struct utsname u;
    if (syscall1(SYS_uname, (long)&u) == 0) {
        print(u.sysname); put_char(' ');
        print(u.release); put_char(' ');
        print(u.machine); put_char('\n');
    }
}

// Process command
static void process(void) {
    const char *p = skip_ws(cmd);
    if (!*p) return;
    
    if (streq(p, "help") || streq(p, "?")) do_help();
    else if (streq(p, "ls")) do_ls(".");
    else if (starts_with(p, "ls ")) do_ls(skip_ws(p + 3));
    else if (streq(p, "cd")) do_cd("/");
    else if (starts_with(p, "cd ")) do_cd(skip_ws(p + 3));
    else if (streq(p, "pwd")) do_pwd();
    else if (starts_with(p, "cat ")) do_cat(skip_ws(p + 4));
    else if (starts_with(p, "touch ")) do_touch(skip_ws(p + 6));
    else if (starts_with(p, "mkdir ")) do_mkdir(skip_ws(p + 6));
    else if (starts_with(p, "mount ")) do_mount(p + 6);
    else if (streq(p, "uname")) do_uname();
    else if (starts_with(p, "echo ")) { print(p + 5); put_char('\n'); }
    else if (streq(p, "exit")) syscall1(SYS_exit, 0);
    else { print("Unknown: "); print(p); put_char('\n'); }
}

void _start_c(void) {
    print("\n");
    print("================================\n");
    print(" OtoRISCV Mini Shell\n");
    print("================================\n\n");
    
    // Mount filesystems
    print("Mounting filesystems...\n");
    if (syscall5(SYS_mount, (long)"none", (long)"/proc", (long)"proc", 0, 0) == 0)
        print("  /proc OK\n");
    if (syscall5(SYS_mount, (long)"none", (long)"/dev", (long)"devtmpfs", 0, 0) == 0)
        print("  /dev OK\n");
    if (syscall5(SYS_mount, (long)"none", (long)"/sys", (long)"sysfs", 0, 0) == 0)
        print("  /sys OK\n");
    
    print("\nType 'help' for commands.\n\n");
    
    // Shell loop
    while (1) {
        print("# ");
        readline();
        process();
    }
}
