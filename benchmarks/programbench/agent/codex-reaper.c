/* codex-reaper: run Codex as a child, reap every process orphaned below it, and end what the
 * node leaves running when Codex exits.
 *
 * Zeroshot 10.8.0's run controller registers as a child subreaper but reaps only its nodes'
 * process groups. A process orphaned in another group (a program started in a pseudo-terminal,
 * with setsid, or in a tmux server) is reparented to the controller and stays a zombie once it
 * exits, until the container's pids limit is exhausted and no tool command can start. As the
 * nearest subreaper below the controller, this process adopts such orphans and reaps them.
 *
 * When Codex exits, the processes the node left running (adopted here, such as a detached tmux
 * server and the programs in it) are killed and reaped before this process exits. Otherwise they
 * would be reparented to the controller, and every orphan they create afterwards would stay a
 * zombie there. Zeroshot already means a node's processes to end with the node (it kills the
 * node's process group); this ends those that left the group.
 *
 * Codex stays in the caller's process group, so Zeroshot's kills (SIGKILL to the group) still
 * reach it, and it is killed if this process dies first. Catchable termination signals sent to
 * the group reach Codex directly; this process ignores them and exits the way Codex exits.
 * Installed execute-only like Codex, so the kernel keeps its environment from the agent.
 */
#define _GNU_SOURCE
#include <ctype.h>
#include <dirent.h>
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#ifndef CODEX
#define CODEX "/opt/codex/bin/codex"
#endif

/* SIGKILL every child of this process; returns how many were signalled. */
static int kill_children(pid_t self) {
    DIR *proc = opendir("/proc");
    struct dirent *entry;
    int signalled = 0;
    if (proc == NULL)
        return 0;
    while ((entry = readdir(proc)) != NULL) {
        char path[300], buf[512];
        const char *end;
        FILE *stat;
        size_t n;
        int ppid;
        if (!isdigit((unsigned char)entry->d_name[0]))
            continue;
        snprintf(path, sizeof path, "/proc/%s/stat", entry->d_name);
        stat = fopen(path, "r");
        if (stat == NULL)
            continue;
        n = fread(buf, 1, sizeof buf - 1, stat);
        fclose(stat);
        buf[n] = '\0';
        end = strrchr(buf, ')'); /* the command name may contain spaces and parentheses */
        if (end != NULL && sscanf(end + 1, " %*c %d", &ppid) == 1 && ppid == self) {
            kill((pid_t)atoi(entry->d_name), SIGKILL);
            signalled++;
        }
    }
    closedir(proc);
    return signalled;
}

/* Kill and reap what the node left running, for at most 10 seconds. A killed process's children
 * are reparented here and killed on the next pass. */
static void end_leftovers(pid_t self) {
    const struct timespec pause = {0, 20 * 1000 * 1000};
    for (int pass = 0; pass < 500; pass++) {
        kill_children(self);
        while (waitpid(-1, NULL, WNOHANG) > 0) {
        }
        if (waitpid(-1, NULL, WNOHANG) < 0 && errno == ECHILD)
            return;
        nanosleep(&pause, NULL);
    }
}

int main(int argc, char *argv[]) {
    static const int ignored[] = {SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGUSR1, SIGUSR2};
    sigset_t all, previous;
    pid_t self = getpid(), child;
    int status = 0;

    (void)argc;
    if (prctl(PR_SET_DUMPABLE, 0, 0, 0, 0) != 0 || prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0) {
        perror("codex-reaper: prctl");
        return 126;
    }
    sigfillset(&all);
    sigprocmask(SIG_BLOCK, &all, &previous);
    child = fork();
    if (child < 0) {
        perror("codex-reaper: fork");
        return 126;
    }
    if (child == 0) {
        if (prctl(PR_SET_PDEATHSIG, SIGKILL, 0, 0, 0) != 0 || getppid() != self)
            _exit(126);
        sigprocmask(SIG_SETMASK, &previous, NULL);
        execv(CODEX, argv);
        perror("codex-reaper: exec " CODEX);
        _exit(127);
    }
    for (size_t i = 0; i < sizeof ignored / sizeof ignored[0]; i++)
        signal(ignored[i], SIG_IGN);
    sigprocmask(SIG_SETMASK, &previous, NULL);

    for (;;) {
        int st;
        pid_t done = waitpid(-1, &st, 0);
        if (done == child) {
            status = st;
            break;
        }
        if (done < 0 && errno == ECHILD)
            break;
    }
    end_leftovers(self);
    if (WIFSIGNALED(status)) {
        int sig = WTERMSIG(status);
        sigset_t only;
        signal(sig, SIG_DFL);
        sigemptyset(&only);
        sigaddset(&only, sig);
        sigprocmask(SIG_UNBLOCK, &only, NULL);
        raise(sig);
        return 128 + sig;
    }
    return WIFEXITED(status) ? WEXITSTATUS(status) : 126;
}
