/*
 * ratchet-candidate-ctr: the setuid-root launcher the evaluator harness uses INSIDE a bench container to start a
 * candidate process as the unprivileged candidate user.
 *
 *     /ratchet/candidate [NAME=value ...] command [args ...]
 *
 * Why it exists: the container image is the repo's own, so it has no sudo and no way to switch users. The harness (trusted
 * code from bench.ref) runs as the holdout uid and owns the holdout file (mode 0600); the candidate's processes (its server)
 * must run as ANOTHER uid so they can neither read the holdout nor signal / ptrace the harness. This binary is static (it
 * works in any image of the same architecture), root-owned and setuid, installed in a root-only directory on the bench host
 * (/usr/local/libexec/ratchet, mode 0700) and bind-mounted READ-ONLY into the confirm and bench harness containers only.
 *
 * What it allows is read from /ratchet/launcher.conf, a root-owned file the bench script writes before each harness
 * container starts and also bind-mounts read-only: "<caller uid> <target uid> <target gid>". It refuses to do anything unless
 * the REAL uid of its caller is the caller uid, the file is a root-owned regular file that nobody else can write, and the
 * target is neither root nor the caller. The container runs with every capability dropped except SETUID and SETGID, so this is
 * the only way the harness can change uid at all.
 *
 * The candidate process gets: no supplementary groups, the target uid/gid everywhere, no_new_privs (so no setuid binary inside
 * the image can give anything back), a clean environment (PATH, HOME, TMPDIR, USER, LOGNAME, LANG and the NAME=value arguments;
 * never RATCHET_TRACE or AWS_*), the checkout as working directory and only stdin / stdout / stderr as open files.
 *
 * Exit status: 70 internal / unexpected error, 77 not allowed, 78 refused argument, 127 command not found (as `env`).
 */
#define _GNU_SOURCE
#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#ifndef CONF_PATH
#define CONF_PATH "/ratchet/launcher.conf"
#endif
#ifndef CANDIDATE_NAME
#define CANDIDATE_NAME "ratchet-bench"
#endif

extern char **environ;

static void die(int code, const char *msg)
{
    fprintf(stderr, "ratchet-candidate: %s\n", msg);
    _exit(code);
}

static int valid_name(const char *s, size_t n)
{
    if (n == 0 || !(isalpha((unsigned char)s[0]) || s[0] == '_'))
        return 0;
    for (size_t i = 1; i < n; i++)
        if (!(isalnum((unsigned char)s[i]) || s[i] == '_'))
            return 0;
    return 1;
}

static const char *need(const char *name)
{
    const char *v = getenv(name);
    if (!v || !*v) {
        fprintf(stderr, "ratchet-candidate: %s is not set\n", name);
        _exit(78);
    }
    return v;
}

static char *kv(const char *k, const char *v)
{
    size_t n = strlen(k) + strlen(v) + 2;
    char *s = malloc(n);
    if (!s)
        die(70, "out of memory");
    snprintf(s, n, "%s=%s", k, v);
    return s;
}

int main(int argc, char **argv)
{
    /* 1. Who may call, and who the candidate becomes: a root-owned file nobody else can write. */
    int fd = open(CONF_PATH, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0)
        die(77, "no launcher configuration (this launcher only works inside a bench container)");
    struct stat st;
    if (fstat(fd, &st) != 0 || !S_ISREG(st.st_mode) || st.st_uid != 0 || (st.st_mode & (S_IWGRP | S_IWOTH)))
        die(77, "the launcher configuration is not a root-owned regular file");
    char buf[128];
    ssize_t got = read(fd, buf, sizeof buf - 1);
    close(fd);
    if (got <= 0)
        die(77, "empty launcher configuration");
    buf[got] = 0;
    unsigned long caller, uid, gid;
    char extra;
    if (sscanf(buf, "%lu %lu %lu %c", &caller, &uid, &gid, &extra) != 3)
        die(77, "malformed launcher configuration");
    if (getuid() != (uid_t)caller)
        die(77, "not allowed: the caller is not the evaluator user");
    if (uid == 0 || gid == 0 || uid == caller)
        die(77, "refusing to run candidate code as root or as the evaluator user");

    /* 2. The environment the candidate gets, built while we still know the harness's values. */
    const char *dir = need("RATCHET_CANDIDATE_DIR");
    const char *home = need("RATCHET_CANDIDATE_HOME");
    const char *tmp = need("RATCHET_CANDIDATE_TMP");
    const char *path = getenv("RATCHET_CANDIDATE_PATH");
    if (!path || !*path)
        path = getenv("PATH");
    if (!path || !*path)
        path = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

    int i = 1;
    int cap = 8 + argc;
    char **envp = calloc((size_t)cap, sizeof *envp);
    if (!envp)
        die(70, "out of memory");
    int n = 0;
    envp[n++] = kv("PATH", path);
    envp[n++] = kv("HOME", home);
    envp[n++] = kv("TMPDIR", tmp);
    envp[n++] = kv("USER", CANDIDATE_NAME);
    envp[n++] = kv("LOGNAME", CANDIDATE_NAME);
    envp[n++] = kv("LANG", "C.UTF-8");
    for (; i < argc; i++) {
        const char *eq = strchr(argv[i], '=');
        if (!eq || !valid_name(argv[i], (size_t)(eq - argv[i])))
            break;
        size_t kn = (size_t)(eq - argv[i]);
        if ((kn == 13 && strncmp(argv[i], "RATCHET_TRACE", 13) == 0) ||
            strncmp(argv[i], "AWS_", 4) == 0) {
            fprintf(stderr, "ratchet-candidate: refusing to pass %.*s to candidate code\n", (int)kn, argv[i]);
            _exit(78);
        }
        envp[n++] = argv[i];
    }
    envp[n] = NULL;
    if (i >= argc)
        die(78, "usage: ratchet-candidate [NAME=value ...] command [args ...]");

    /* 3. Become the candidate for good: groups, gid, uid (real, effective, saved), and no way back. */
    if (setgroups(0, NULL) != 0)
        die(70, "setgroups failed");
    if (setresgid((gid_t)gid, (gid_t)gid, (gid_t)gid) != 0)
        die(70, "setresgid failed");
    if (setresuid((uid_t)uid, (uid_t)uid, (uid_t)uid) != 0)
        die(70, "setresuid failed");
    if (getuid() != (uid_t)uid || geteuid() != (uid_t)uid || getgid() != (gid_t)gid || getegid() != (gid_t)gid)
        die(70, "the identity switch did not take effect");
    if (setuid(0) == 0 || seteuid(0) == 0)
        die(70, "the process could regain root");
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0)
        die(70, "cannot set no_new_privs");

    /* 4. Nothing inherited but the standard streams, then the candidate's checkout. */
    long maxfd = sysconf(_SC_OPEN_MAX);
    if (maxfd < 0 || maxfd > 65536)
        maxfd = 65536;
    for (int f = 3; f < maxfd; f++)
        close(f);
    if (chdir(dir) != 0) {
        fprintf(stderr, "ratchet-candidate: cannot enter %s: %s\n", dir, strerror(errno));
        _exit(78);
    }
    environ = envp;
    execvp(argv[i], &argv[i]);
    fprintf(stderr, "ratchet-candidate: cannot run %s: %s\n", argv[i], strerror(errno));
    _exit(errno == ENOENT ? 127 : 126);
}
