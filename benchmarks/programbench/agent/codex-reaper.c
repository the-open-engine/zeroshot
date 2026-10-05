/* codex-reaper: run Codex as a child and reap every process orphaned below it.
 *
 * Zeroshot 10.8.0's run controller registers as a child subreaper but reaps only its nodes'
 * process groups. A process orphaned in another group (a program started in a pseudo-terminal
 * or with setsid) is reparented to the controller and stays a zombie once it exits, until the
 * container's pids limit is exhausted and no tool command can start. As the nearest subreaper
 * below the controller, this process adopts such orphans and reaps them instead.
 *
 * Codex stays in the caller's process group, so Zeroshot's kills (SIGKILL to the group) still
 * reach it, and it is killed if this process dies first. Catchable termination signals sent to
 * the group reach Codex directly; this process ignores them and exits the way Codex exits.
 * Installed execute-only like Codex, so the kernel keeps its environment from the agent.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <sys/prctl.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef CODEX
#define CODEX "/opt/codex/bin/codex"
#endif

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
    while (waitpid(-1, NULL, WNOHANG) > 0) {
    }
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
