#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

/* Pause a real files-backend Git writer immediately before its loose-ref lock.
 * File signalling keeps the Rust test free of inherited-FD or pre_exec hooks. */
static atomic_int paused;
static void gate(const char *path, int flags, const char *phase) {
    const char *wanted = getenv("BUILD_RECEIVER_GATE_PHASE");
    if (!wanted) wanted = "before";
    if (strcmp(wanted, phase)) return;
    const char *suffix = getenv("BUILD_RECEIVER_GATE_SUFFIX");
    if (!(flags & O_CREAT) || !path || !suffix || strlen(path) < strlen(suffix)
        || strcmp(path + strlen(path) - strlen(suffix), suffix)) return;
    if (atomic_exchange(&paused, 1)) return;
    const char *ready = getenv("BUILD_RECEIVER_GATE_READY");
    const char *release = getenv("BUILD_RECEIVER_GATE_RELEASE");
    if (!ready || !release) _exit(85);
    int (*real_open)(const char *, int, ...) = dlsym(RTLD_NEXT, "open");
    int fd = real_open(ready, O_CREAT | O_EXCL | O_WRONLY, 0600);
    if (fd < 0) _exit(86);
    if (write(fd, "ready\n", 6) != 6) _exit(87);
    close(fd);
    const struct timespec pause = { .tv_sec = 0, .tv_nsec = 10000000 };
    for (int attempt = 0; attempt < 500; attempt++) {
        if (access(release, F_OK) == 0) return;
        nanosleep(&pause, NULL);
    }
    _exit(88);
}

#define OPEN_WRAPPER(name) \
int name(const char *path, int flags, ...) { \
    mode_t mode = 0; \
    if (flags & O_CREAT) { va_list args; va_start(args, flags); mode = va_arg(args, int); va_end(args); } \
    gate(path, flags, "before"); \
    int result = ((int (*)(const char *, int, ...))dlsym(RTLD_NEXT, #name))(path, flags, mode); \
    if (result >= 0) gate(path, flags, "after"); \
    return result; \
}
#define OPENAT_WRAPPER(name) \
int name(int directory, const char *path, int flags, ...) { \
    mode_t mode = 0; \
    if (flags & O_CREAT) { va_list args; va_start(args, flags); mode = va_arg(args, int); va_end(args); } \
    gate(path, flags, "before"); \
    int result = ((int (*)(int, const char *, int, ...))dlsym(RTLD_NEXT, #name))(directory, path, flags, mode); \
    if (result >= 0) gate(path, flags, "after"); \
    return result; \
}
OPEN_WRAPPER(open)
OPEN_WRAPPER(open64)
OPENAT_WRAPPER(openat)
OPENAT_WRAPPER(openat64)
