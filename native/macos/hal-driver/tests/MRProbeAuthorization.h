#ifndef MR_PROBE_AUTHORIZATION_H
#define MR_PROBE_AUTHORIZATION_H

#include <stdbool.h>
#include <stdio.h>

typedef struct MRInputAuthorization {
    long initial, final;
    bool requested;
} MRInputAuthorization;

bool mr_authorize_input(double timeout, MRInputAuthorization *result);
void mr_print_input_authorization(FILE *stream, const MRInputAuthorization *result);
int mr_authorization_command(int argc, char **argv);

#endif
