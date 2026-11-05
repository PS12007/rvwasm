// rvwasm mini init: a tiny interactive shell for the Linux demo, built with
// no libc (raw RISC-V Linux syscalls), so it runs on RV64IMA without any
// FP or compressed instructions. When BusyBox is in the initramfs, this
// program is not used.
//
// Commands: help, uname, echo, ls [dir], cat FILE, mounts, mem, uptime,
//           mkdir DIR, write FILE TEXT, clear, poweroff

typedef unsigned long u64;
typedef long i64;

#define SYS_getcwd 17
#define SYS_ioctl 29
#define SYS_mkdirat 34
#define SYS_mount 40
#define SYS_openat 56
#define SYS_close 57
#define SYS_getdents64 61
#define SYS_read 63
#define SYS_write 64
#define SYS_exit 93
#define SYS_reboot 142
#define SYS_uname 160
#define SYS_sysinfo 179
#define AT_FDCWD (-100)
#define O_RDONLY 0
#define O_WRONLY 1
#define O_CREAT 0100
#define O_TRUNC 01000
#define O_DIRECTORY 0200000

static i64 sys6(i64 n, i64 a, i64 b, i64 c, i64 d, i64 e, i64 f) {
  register i64 a0 asm("a0") = a;
  register i64 a1 asm("a1") = b;
  register i64 a2 asm("a2") = c;
  register i64 a3 asm("a3") = d;
  register i64 a4 asm("a4") = e;
  register i64 a5 asm("a5") = f;
  register i64 a7 asm("a7") = n;
  asm volatile("ecall" : "+r"(a0) : "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a5), "r"(a7) : "memory");
  return a0;
}
#define sys3(n, a, b, c) sys6(n, (i64)(a), (i64)(b), (i64)(c), 0, 0, 0)

static u64 slen(const char *s) { u64 n = 0; while (s[n]) n++; return n; }
static int seq(const char *a, const char *b) { while (*a && *a == *b) a++, b++; return *a == *b; }
static void out(const char *s) { sys3(SYS_write, 1, s, slen(s)); }
static void outn(const char *s, u64 n) { sys3(SYS_write, 1, s, n); }

static void outu(u64 v) {
  char b[24]; int i = 23; b[i] = 0;
  do { b[--i] = '0' + v % 10; v /= 10; } while (v);
  out(b + i);
}

struct utsname { char sysname[65], nodename[65], release[65], version[65], machine[65], domain[65]; };
struct sysinfo_s { i64 uptime; u64 loads[3]; u64 totalram, freeram, sharedram, bufferram, totalswap, freeswap;
  unsigned short procs, pad; u64 totalhigh, freehigh; unsigned mem_unit; char f[8]; };

static void cmd_ls(const char *dir) {
  int fd = sys3(SYS_openat, AT_FDCWD, dir, O_RDONLY | O_DIRECTORY);
  if (fd < 0) { out("ls: cannot open\n"); return; }
  static char buf[4096];
  for (;;) {
    i64 n = sys3(SYS_getdents64, fd, buf, sizeof buf);
    if (n <= 0) break;
    for (i64 off = 0; off < n;) {
      unsigned short reclen = *(unsigned short *)(buf + off + 16);
      unsigned char type = *(unsigned char *)(buf + off + 18);
      const char *name = buf + off + 19;
      if (name[0] != '.') { out(name); out(type == 4 ? "/  " : "  "); }
      off += reclen;
    }
  }
  out("\n");
  sys3(SYS_close, fd, 0, 0);
}

static void cmd_cat(const char *path) {
  int fd = sys3(SYS_openat, AT_FDCWD, path, O_RDONLY);
  if (fd < 0) { out("cat: cannot open\n"); return; }
  static char buf[1024];
  i64 n;
  while ((n = sys3(SYS_read, fd, buf, sizeof buf)) > 0) outn(buf, n);
  sys3(SYS_close, fd, 0, 0);
}

static int readline(char *buf, int max) {
  int n = 0;
  for (;;) {
    char c;
    if (sys3(SYS_read, 0, &c, 1) <= 0) return -1;
    if (c == '\r' || c == '\n') { out("\n"); buf[n] = 0; return n; }
    if (c == 0x7f || c == 8) { if (n) { n--; out("\b \b"); } continue; }
    if (c == 3) { out("^C\n"); buf[0] = 0; return 0; }
    if (n < max - 1 && c >= 32) { buf[n++] = c; outn(&c, 1); }
  }
}

// Split on the first space: returns the argument (or "").
static char *split(char *s) {
  while (*s && *s != ' ') s++;
  if (!*s) return s;
  *s++ = 0;
  while (*s == ' ') s++;
  return s;
}

// Turn off the tty's canonical mode and echo: this shell does its own line editing.
static void raw_tty(void) {
  struct { unsigned iflag, oflag, cflag, lflag; unsigned char line, cc[19]; } t;
  if (sys3(SYS_ioctl, 0, 0x5401 /* TCGETS */, &t) < 0) return;
  t.lflag &= ~(0x2 /* ICANON */ | 0x8 /* ECHO */);
  t.cc[6] = 1;  // VMIN
  t.cc[5] = 0;  // VTIME
  sys3(SYS_ioctl, 0, 0x5402 /* TCSETS */, &t);
}

void _start_c(void) {
  raw_tty();
  sys6(SYS_mount, (i64)"proc", (i64)"/proc", (i64)"proc", 0, 0, 0);
  sys6(SYS_mount, (i64)"sysfs", (i64)"/sys", (i64)"sysfs", 0, 0, 0);
  sys6(SYS_mount, (i64)"tmpfs", (i64)"/tmp", (i64)"tmpfs", 0, 0, 0);

  struct utsname u;
  sys3(SYS_uname, &u, 0, 0);
  out("\n\033[1mrvwasm mini init\033[0m on Linux "); out(u.release); out(" ("); out(u.machine); out(")\n");
  out("BusyBox is not in this initramfs; this is a tiny built-in shell. Type 'help'.\n\n");

  static char line[256];
  for (;;) {
    out("\033[32m#\033[0m ");
    if (readline(line, sizeof line) < 0) continue;
    char *arg = split(line);
    if (!line[0]) continue;
    if (seq(line, "help")) {
      out("help uname echo ls [dir] cat FILE mounts mem uptime mkdir DIR write FILE TEXT clear poweroff\n");
    } else if (seq(line, "uname")) {
      out(u.sysname); out(" "); out(u.nodename); out(" "); out(u.release); out(" "); out(u.version); out(" ");
      out(u.machine); out("\n");
    } else if (seq(line, "echo")) {
      out(arg); out("\n");
    } else if (seq(line, "ls")) {
      cmd_ls(arg[0] ? arg : ".");
    } else if (seq(line, "cat")) {
      cmd_cat(arg);
    } else if (seq(line, "mounts")) {
      cmd_cat("/proc/mounts");
    } else if (seq(line, "mem") || seq(line, "uptime")) {
      struct sysinfo_s si;
      sys3(SYS_sysinfo, &si, 0, 0);
      if (line[0] == 'm') {
        out("total "); outu(si.totalram * si.mem_unit / 1024); out(" KiB, free ");
        outu(si.freeram * si.mem_unit / 1024); out(" KiB, procs "); outu(si.procs); out("\n");
      } else {
        out("up "); outu(si.uptime); out(" s\n");
      }
    } else if (seq(line, "mkdir")) {
      if (sys3(SYS_mkdirat, AT_FDCWD, arg, 0755) < 0) out("mkdir: failed\n");
    } else if (seq(line, "write")) {
      char *text = split(arg);
      int fd = sys6(SYS_openat, AT_FDCWD, (i64)arg, O_WRONLY | O_CREAT | O_TRUNC, 0644, 0, 0);
      if (fd < 0) { out("write: cannot open\n"); continue; }
      sys3(SYS_write, fd, text, slen(text));
      sys3(SYS_write, fd, "\n", 1);
      sys3(SYS_close, fd, 0, 0);
    } else if (seq(line, "clear")) {
      out("\033[2J\033[H");
    } else if (seq(line, "poweroff")) {
      sys6(SYS_reboot, 0xfee1dead, 672274793, 0x4321fedc, 0, 0, 0);
    } else {
      out(line); out(": unknown command (try 'help')\n");
    }
  }
}

asm(".pushsection .text\n.globl _start\n_start:\n  la sp, stack_top\n  call _start_c\n1: j 1b\n.popsection\n"
    ".pushsection .bss\n.balign 16\nstack: .skip 16384\nstack_top:\n.popsection\n");
