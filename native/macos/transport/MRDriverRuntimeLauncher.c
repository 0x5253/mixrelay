#include "../installer/MRInstallContract.h"
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

static volatile sig_atomic_t stopping;

static void request_stop(int signal_number) {
    (void)signal_number;
    stopping = 1;
}

static void child_changed(int signal_number) { (void)signal_number; }

// Keep this non-UI process in the editor's background coalition. Execing the
// Driver here would transfer its identity to the separate menu-bar app and
// remove the editor's Dock background indicator. Driver updates stay separate.
int main(int argc, char **argv) {
    (void)argv;
    if (argc != 1) return 64;

    sigset_t watched, previous;
    sigemptyset(&watched);
    sigaddset(&watched, SIGTERM);
    sigaddset(&watched, SIGINT);
    sigaddset(&watched, SIGHUP);
    sigaddset(&watched, SIGCHLD);
    if (sigprocmask(SIG_BLOCK, &watched, &previous) != 0) return 71;
    // These lifecycle signals must be delivered even if the launching thread
    // inherited a blocked mask; preserve the other inherited signal settings.
    sigdelset(&previous, SIGTERM);
    sigdelset(&previous, SIGINT);
    sigdelset(&previous, SIGHUP);
    sigdelset(&previous, SIGCHLD);
    struct sigaction action = {0};
    sigemptyset(&action.sa_mask);
    action.sa_handler = request_stop;
    if (sigaction(SIGTERM, &action, NULL) != 0 ||
        sigaction(SIGINT, &action, NULL) != 0 ||
        sigaction(SIGHUP, &action, NULL) != 0) return 71;
    action.sa_handler = child_changed;
    if (sigaction(SIGCHLD, &action, NULL) != 0) return 71;

    pid_t child = fork();
    if (child < 0) return 71;
    if (child == 0) {
        action.sa_handler = SIG_DFL;
        sigaction(SIGTERM, &action, NULL);
        sigaction(SIGINT, &action, NULL);
        sigaction(SIGHUP, &action, NULL);
        sigaction(SIGCHLD, &action, NULL);
        sigprocmask(SIG_SETMASK, &previous, NULL);
        const char *program = MR_APP_EXECUTABLE_PATH;
        char *const arguments[] = {(char *)program, "--driver-runtime", NULL};
        execv(program, arguments);
        perror("MixRelay Driver runtime could not start");
        _exit(71);
    }

    int status = 0;
    int forwarded = 0;
    for (;;) {
        if (stopping && !forwarded) {
            if (kill(child, SIGTERM) != 0 && errno != ESRCH)
                perror("MixRelay Driver runtime could not stop");
            forwarded = 1;
        }
        pid_t result = waitpid(child, &status, WNOHANG);
        if (result == child) break;
        if (result < 0 && errno != EINTR) return 71;
        // Watched signals stay blocked between the flag check and waitpid.
        // sigsuspend atomically unmasks them, including a pending child exit.
        sigsuspend(&previous);
    }
    return WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
}
