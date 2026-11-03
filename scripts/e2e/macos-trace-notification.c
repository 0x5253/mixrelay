// Observe xctrace's documented Darwin start notification before measuring PCM.
#include <dispatch/dispatch.h>
#include <notify.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    if (argc != 2 || argv[1][0] == '\0') return 64;
    int token = 0;
    uint32_t status = notify_register_dispatch(argv[1], &token, dispatch_get_main_queue(), ^(int received) {
        (void)notify_cancel(received);
        puts("started");
        fflush(stdout);
        exit(0);
    });
    if (status != NOTIFY_STATUS_OK) {
        fprintf(stderr, "Darwin notification registration failed: %u\n", status);
        return 1;
    }
    puts("registered");
    fflush(stdout);
    dispatch_main();
}
