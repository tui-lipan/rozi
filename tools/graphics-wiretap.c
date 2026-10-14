/* Optional Linux benchmark interposer: record successful owned TTY writes. */
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
static ssize_t (*real_write)(int,const void*,size_t);
static ssize_t (*real_writev)(int,const struct iovec*,int);
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static int output = -1, checked;
static int is_host_tty(int fd) {
    struct stat actual, standard;
    if (!isatty(fd) || !isatty(1)) return 0;
    if (fstat(fd, &actual) || fstat(1, &standard)) return 0;
    return actual.st_rdev == standard.st_rdev && actual.st_ino == standard.st_ino;
}
static void open_sink(void) {
    checked = 1;
    const char *target = getenv("GRAPHICS_WIRETAP_EXE"), *path = getenv("GRAPHICS_WIRETAP_PATH");
    if (!target || !path) return;
    char executable[PATH_MAX];
    ssize_t n = readlink("/proc/self/exe", executable, sizeof(executable)-1);
    if (n <= 0) return;
    executable[n] = 0;
    if (strcmp(target, executable) == 0)
        output = open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600);
}
static void record(int fd, const void *bytes, size_t length) {
    if (!is_host_tty(fd)) return;
    pthread_mutex_lock(&lock);
    if (!checked) open_sink();
    if (output >= 0) {
        struct timespec now; clock_gettime(CLOCK_MONOTONIC, &now);
        uint64_t ns = (uint64_t)now.tv_sec * 1000000000ull + now.tv_nsec;
        uint32_t count = length;
        real_write(output, &ns, sizeof(ns)); real_write(output, &count, sizeof(count));
        real_write(output, bytes, length);
    }
    pthread_mutex_unlock(&lock);
}
ssize_t write(int fd, const void *bytes, size_t length) {
    if (!real_write) real_write = dlsym(RTLD_NEXT, "write");
    ssize_t n = real_write(fd, bytes, length); if (n > 0) record(fd, bytes, n); return n;
}
ssize_t writev(int fd, const struct iovec *iov, int count) {
    if (!real_writev) real_writev = dlsym(RTLD_NEXT, "writev");
    if (!real_write) real_write = dlsym(RTLD_NEXT, "write");
    ssize_t n = real_writev(fd, iov, count); size_t left = n > 0 ? n : 0;
    for (int i = 0; i < count && left; ++i) {
        size_t length = iov[i].iov_len < left ? iov[i].iov_len : left;
        record(fd, iov[i].iov_base, length); left -= length;
    }
    return n;
}
