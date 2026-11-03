#ifndef MR_HAL_CONTROL_H
#define MR_HAL_CONTROL_H
#include "MRHALCore.h"
/* Writes the caller's registry slot before publishing asynchronous work.
 * False leaves that slot unchanged and publishes no callback; the caller may retry. */
bool mr_hal_control_initialize(const MRHALCoreHostOps *host, MRHALCore **output);
#endif
