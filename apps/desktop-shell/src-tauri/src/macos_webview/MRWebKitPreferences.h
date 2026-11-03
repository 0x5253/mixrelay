#pragma once
#include <stdbool.h>
#include <stdint.h>

// The caller retains the main-thread WKPreferences and a terminated UTF-8 key.
int32_t mr_webkit_set_bool_feature(void *preferences, const char *key, bool enabled);
