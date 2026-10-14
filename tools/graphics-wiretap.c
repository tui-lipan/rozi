/* Optional Linux benchmark interposer: record successful owned TTY writes.
 *
 * GRAPHICS_WIRETAP_PATH receives u64 CLOCK_MONOTONIC ns, u32 length and the written bytes.
 * GRAPHICS_WIRETAP_MEDIA_PATH, when set, receives u64 ns, the first four bytes of each file a
 * complete file upload (`t=t` or `t=f`) names, and u32 1 when they were read (0 otherwise). Files are read before
 * the write that hands them to the host, which may delete them as soon as it sees the path.
 *
 * Only the executable named by GRAPHICS_WIRETAP_EXE is measured. Every other process that loads
 * this library passes calls straight through, and the measured process removes LD_PRELOAD from
 * its environment, so its children (the session server, panes and their producers) never load it.
 * A producer's sandboxed helpers may forbid even the terminal ioctl behind isatty().
 *
 * GRAPHICS_WIRETAP_READ_EXE instead measures the host terminal: in that executable, reads from a
 * PTY master go to GRAPHICS_WIRETAP_READ_PATH (and marks to GRAPHICS_WIRETAP_READ_MEDIA_PATH) in
 * the same formats, timestamped on receipt. It sees any multiplexer's output, including programs
 * that bypass libc, and keeps LD_PRELOAD for the programs it starts. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <sys/uio.h>
#include <sys/stat.h>

#define MAX_MARKS 64

struct mark { size_t end; unsigned char pixel[4]; uint32_t read; };

static ssize_t (*real_write)(int,const void*,size_t);
static ssize_t (*real_writev)(int,const struct iovec*,int);
static ssize_t (*real_read)(int,void*,size_t);
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static int output = -1, media = -1, read_output = -1, read_media = -1;

static int is_executable(const char *target) {
    char executable[PATH_MAX];
    ssize_t n = readlink("/proc/self/exe", executable, sizeof(executable)-1);
    if (!target || n <= 0) return 0;
    executable[n] = 0;
    return strcmp(target, executable) == 0;
}

static int open_record(const char *path) {
    return path ? open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600) : -1;
}

__attribute__((constructor)) static void open_sink(void) {
    if (is_executable(getenv("GRAPHICS_WIRETAP_READ_EXE"))) {
        read_output = open_record(getenv("GRAPHICS_WIRETAP_READ_PATH"));
        if (read_output >= 0) read_media = open_record(getenv("GRAPHICS_WIRETAP_READ_MEDIA_PATH"));
        return;
    }
    const char *path = getenv("GRAPHICS_WIRETAP_PATH");
    if (!path || !is_executable(getenv("GRAPHICS_WIRETAP_EXE"))) return;
    unsetenv("LD_PRELOAD");
    output = open_record(path);
    if (output >= 0) media = open_record(getenv("GRAPHICS_WIRETAP_MEDIA_PATH"));
}

static int is_pty_master(int fd) {
    char link[64], target[32];
    snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
    ssize_t n = readlink(link, target, sizeof(target)-1);
    if (n <= 0) return 0;
    target[n] = 0;
    return strcmp(target, "/dev/ptmx") == 0;
}

static int is_host_tty(int fd) {
    struct stat actual, standard;
    if (!isatty(fd) || !isatty(1)) return 0;
    if (fstat(fd, &actual) || fstat(1, &standard)) return 0;
    return actual.st_rdev == standard.st_rdev && actual.st_ino == standard.st_ino;
}

static uint64_t now_ns(void) {
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return (uint64_t)now.tv_sec * 1000000000ull + now.tv_nsec;
}

static int base64_value(unsigned char c) {
    if (c >= 'A' && c <= 'Z') return c - 'A';
    if (c >= 'a' && c <= 'z') return c - 'a' + 26;
    if (c >= '0' && c <= '9') return c - '0' + 52;
    if (c == '+') return 62;
    if (c == '/') return 63;
    return -1;
}

static int decode_path(const unsigned char *in, size_t length, char *path) {
    size_t used = 0;
    uint32_t bits = 0;
    int count = 0;
    for (size_t i = 0; i < length && in[i] != '='; ++i) {
        int value = base64_value(in[i]);
        if (value < 0) return 0;
        bits = bits << 6 | (uint32_t)value;
        if ((count += 6) >= 8) {
            count -= 8;
            if (used + 1 >= PATH_MAX) return 0;
            path[used++] = (char)(bits >> count & 0xFF);
        }
    }
    path[used] = 0;
    return used > 0;
}

/* The control data is comma-separated keys; the medium must be a whole key, not a substring. */
static int names_a_file(const unsigned char *control, size_t length) {
    for (size_t start = 0; start < length;) {
        size_t end = start;
        while (end < length && control[end] != ',') ++end;
        if (end - start == 3 && (memcmp(control + start, "t=t", 3) == 0
                                 || memcmp(control + start, "t=f", 3) == 0)) return 1;
        start = end + 1;
    }
    return 0;
}

static size_t scan_marks(const unsigned char *bytes, size_t length, size_t base,
                         struct mark *marks, size_t count) {
    for (size_t i = 0; i + 2 < length && count < MAX_MARKS; ++i) {
        if (bytes[i] != 0x1b || bytes[i+1] != '_' || bytes[i+2] != 'G') continue;
        size_t semi = i + 3;
        while (semi < length && bytes[semi] != ';' && bytes[semi] != 0x1b) ++semi;
        if (semi >= length || bytes[semi] != ';' || !names_a_file(bytes + i + 3, semi - i - 3))
            continue;
        size_t end = semi + 1;
        while (end + 1 < length && !(bytes[end] == 0x1b && bytes[end+1] == '\\')) ++end;
        if (end + 1 >= length) break;
        struct mark *mark = &marks[count++];
        char path[PATH_MAX];
        memset(mark, 0, sizeof(*mark));
        mark->end = base + end + 2;
        if (decode_path(bytes + semi + 1, end - semi - 1, path)) {
            int file = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK);
            if (file >= 0) {
                mark->read = pread(file, mark->pixel, sizeof(mark->pixel), 0) == 4;
                close(file);
            }
        }
        i = end + 1;
    }
    return count;
}

/* One timestamp per successful write, shared by its wire and media records, so an upload and the
 * frame end written with it compare equal. */
static void record(int sink, uint64_t ns, const void *bytes, size_t length) {
    pthread_mutex_lock(&lock);
    if (sink >= 0) {
        uint32_t count = length;
        real_write(sink, &ns, sizeof(ns)); real_write(sink, &count, sizeof(count));
        real_write(sink, bytes, length);
    }
    pthread_mutex_unlock(&lock);
}

static void record_marks(int sink, uint64_t ns, const struct mark *marks, size_t count,
                         size_t written) {
    pthread_mutex_lock(&lock);
    if (sink >= 0) {
        for (size_t i = 0; i < count && marks[i].end <= written; ++i) {
            real_write(sink, &ns, sizeof(ns));
            real_write(sink, marks[i].pixel, sizeof(marks[i].pixel));
            real_write(sink, &marks[i].read, sizeof(marks[i].read));
        }
    }
    pthread_mutex_unlock(&lock);
}

ssize_t write(int fd, const void *bytes, size_t length) {
    if (!real_write) real_write = dlsym(RTLD_NEXT, "write");
    int owned = output >= 0 && is_host_tty(fd);
    struct mark marks[MAX_MARKS];
    size_t count = owned && media >= 0 ? scan_marks(bytes, length, 0, marks, 0) : 0;
    ssize_t n = real_write(fd, bytes, length);
    if (owned && n > 0) {
        uint64_t ns = now_ns();
        record(output, ns, bytes, n);
        record_marks(media, ns, marks, count, n);
    }
    return n;
}

ssize_t writev(int fd, const struct iovec *iov, int count) {
    if (!real_writev) real_writev = dlsym(RTLD_NEXT, "writev");
    if (!real_write) real_write = dlsym(RTLD_NEXT, "write");
    int owned = output >= 0 && is_host_tty(fd);
    struct mark marks[MAX_MARKS];
    size_t found = 0, offset = 0;
    for (int i = 0; owned && media >= 0 && i < count; ++i) {
        found = scan_marks(iov[i].iov_base, iov[i].iov_len, offset, marks, found);
        offset += iov[i].iov_len;
    }
    ssize_t n = real_writev(fd, iov, count);
    if (owned && n > 0) {
        uint64_t ns = now_ns();
        size_t left = n;
        for (int i = 0; i < count && left; ++i) {
            size_t length = iov[i].iov_len < left ? iov[i].iov_len : left;
            record(output, ns, iov[i].iov_base, length); left -= length;
        }
        record_marks(media, ns, marks, found, n);
    }
    return n;
}

ssize_t read(int fd, void *bytes, size_t length) {
    if (!real_read) real_read = dlsym(RTLD_NEXT, "read");
    if (!real_write) real_write = dlsym(RTLD_NEXT, "write");
    ssize_t n = real_read(fd, bytes, length);
    if (read_output < 0 || n <= 0 || !is_pty_master(fd)) return n;
    uint64_t ns = now_ns();
    struct mark marks[MAX_MARKS];
    size_t count = read_media >= 0 ? scan_marks(bytes, n, 0, marks, 0) : 0;
    record(read_output, ns, bytes, n);
    record_marks(read_media, ns, marks, count, n);
    return n;
}
