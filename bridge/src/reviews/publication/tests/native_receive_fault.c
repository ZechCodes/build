#define _GNU_SOURCE
#include <dlfcn.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Test-only fault at Git's native receiving-ref commit boundary. Build's
 * registered writer publishes a different temporary filename instead. */
int rename(const char *from, const char *to) {
    int (*real_rename)(const char *, const char *) = dlsym(RTLD_NEXT, "rename");
    const char *lock_suffix = getenv("BUILD_REVIEW_NATIVE_REF_LOCK");
    const char *trace_path = getenv("BUILD_REVIEW_NATIVE_REF_TRACE");
    if (lock_suffix && trace_path && strstr(from, lock_suffix)) {
        FILE *trace = fopen(trace_path, "a");
        if (trace) {
            fprintf(trace, "native ref write: %s -> %s\n", from, to);
            fflush(trace);
            fclose(trace);
        }
        raise(SIGKILL);
    }
    return real_rename(from, to);
}
