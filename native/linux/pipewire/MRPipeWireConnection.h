#ifndef MR_PIPEWIRE_CONNECTION_H
#define MR_PIPEWIRE_CONNECTION_H
#include <stdint.h>

typedef struct MRPWConnection MRPWConnection;
struct pw_core;
struct pw_thread_loop;
typedef struct {
    struct pw_core *core;
    struct pw_thread_loop *thread_loop;
} MRPWConnectionHandles;

/* Rust owns loop/context/core and selects the installed audio-client config.
 * Open prepares without starting. All children must close before connection
 * close, and all control calls after start must lock the borrowed loop.
 * No process environment, daemon defaults or scheduling privileges are changed. */
MRPWConnection *mr_pw_connection_open(const char *name);
MRPWConnectionHandles mr_pw_connection_handles(const MRPWConnection *owner);
int mr_pw_connection_start(MRPWConnection *owner);
/* After export, before activation: retain the exact inactive capture through
 * bounded manager/DSP-monitor preparation. No concurrent control calls or RT. */
int mr_pw_connection_prepare_capture(MRPWConnection *owner, uint32_t node_id, uint32_t rate);
void mr_pw_connection_close(MRPWConnection *owner);
#endif
