/* Optional Linux benchmark interposer: record successful owned TTY writes.
 *
 * GRAPHICS_WIRETAP_PATH receives u64 CLOCK_MONOTONIC ns, u32 length and the written bytes.
 * GRAPHICS_WIRETAP_MEDIA_PATH, when set, receives u64 ns, the first four bytes of each file a
 * complete `t=t` upload names, and u32 1 when they were read (0 otherwise). Files are read before
 * the write that hands them to the host, which may delete them as soon as it sees the path. */
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
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static int output = -1, media = -1, checked;

static void open_sink(void) {
    checked = 1;
    const char *target = getenv("GRAPHICS_WIRETAP_EXE"), *path = getenv("GRAPHICS_WIRETAP_PATH");
    const char *media_path = getenv("GRAPHICS_WIRETAP_MEDIA_PATH");
    if (!target || !path) return;
    char executable[PATH_MAX];
    ssize_t n = readlink("/proc/self/exe", executable, sizeof(executable)-1);
    if (n <= 0) return;
    executable[n] = 0;
    if (strcmp(target, executable) != 0) return;
    output = open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600);
    if (media_path) media = open(media_path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600);
}

static int sink_enabled(void) {
    pthread_mutex_lock(&lock);
    if (!checked) open_sink();
    int enabled = output >= 0;
    pthread_mutex_unlock(&lock);
    return enabled;
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

/* The control data is comma-separated keys; `t=t` must be a whole key, not a substring. */
static int uses_temp_file(const unsigned char *control, size_t length) {
    for (size_t start = 0; start < length;) {
        size_t end = start;
        while (end < length && control[end] != ',') ++end;
        if (end - start == 3 && memcmp(control + start, "t=t", 3) == 0) return 1;
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
        if (semi >= length || bytes[semi] != ';' || !uses_temp_file(bytes + i + 3, semi - i - 3))
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
static void record(uint64_t ns, const void *bytes, size_t length) {
    pthread_mutex_lock(&lock);
    if (output >= 0) {
        uint32_t count = length;
        real_write(output, &ns, sizeof(ns)); real_write(output, &count, sizeof(count));
        real_write(output, bytes, length);
    }
    pthread_mutex_unlock(&lock);
}

static void record_marks(uint64_t ns, const struct mark *marks, size_t count, size_t written) {
    pthread_mutex_lock(&lock);
    if (media >= 0) {
        for (size_t i = 0; i < count && marks[i].end <= written; ++i) {
            real_write(media, &ns, sizeof(ns));
            real_write(media, marks[i].pixel, sizeof(marks[i].pixel));
            real_write(media, &marks[i].read, sizeof(marks[i].read));
        }
    }
    pthread_mutex_unlock(&lock);
}

ssize_t write(int fd, const void *bytes, size_t length) {
    if (!real_write) real_write = dlsym(RTLD_NEXT, "write");
    int owned = is_host_tty(fd) && sink_enabled();
    struct mark marks[MAX_MARKS];
    size_t count = owned && media >= 0 ? scan_marks(bytes, length, 0, marks, 0) : 0;
    ssize_t n = real_write(fd, bytes, length);
    if (owned && n > 0) {
        uint64_t ns = now_ns();
        record(ns, bytes, n);
        record_marks(ns, marks, count, n);
    }
    return n;
}

ssize_t writev(int fd, const struct iovec *iov, int count) {
    if (!real_writev) real_writev = dlsym(RTLD_NEXT, "writev");
    if (!real_write) real_write = dlsym(RTLD_NEXT, "write");
    int owned = is_host_tty(fd) && sink_enabled();
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
            record(ns, iov[i].iov_base, length); left -= length;
        }
        record_marks(ns, marks, found, n);
    }
    return n;
}
