#ifndef MR_HAL_IO_H
#define MR_HAL_IO_H

#include <stdbool.h>
#include <stdint.h>

/* Converts Core Audio's integral sample-time domain without losing precision. */
bool mr_hal_io_sample_frame(double sample_time, uint64_t *sample_frame);

#endif
