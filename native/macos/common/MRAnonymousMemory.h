#ifndef MR_ANONYMOUS_MEMORY_H
#define MR_ANONYMOUS_MEMORY_H

#include <stdbool.h>
#include <stddef.h>

/* Rust-owned POSIX shared-memory object, unlinked before return. No filesystem
 * file is created. Caller owns the returned FD; -1 has no acquired resources. */
int mr_shared_memory_create(size_t size, bool writable);

#endif
