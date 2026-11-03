#ifndef MR_AGGREGATE_GROUP_H
#define MR_AGGREGATE_GROUP_H
#include <stdbool.h>
#include <stdint.h>
typedef struct MRAggregateGroup MRAggregateGroup;
/* Native facts only. Rust owns compatibility, uniqueness and channel budgets. */
typedef struct MRAggregateMemberInfo {
    uint32_t device, transport;
    double rate;
    uint32_t input, output, alive;
} MRAggregateMemberInfo;
_Static_assert(sizeof(MRAggregateMemberInfo) == 32, "Rust member snapshot ABI");
bool mr_aggregate_member_info(const char *uid, MRAggregateMemberInfo *info);
/* Control-thread API. The caller supplies a validated Rust plan (unique fixed
 * UIDs/devices, compatible integral rates, bounded channel totals). Rust geometry
 * verifies freshly queried identity/layout before creation, not compatibility.
 * This never rewrites device rates, defaults or user-created aggregates.
 * status=1 means invalid ABI arguments,
 * status=2 means an attempted native operation failed (not a silent fallback).
 * ANY non-NULL return transfers ownership, including partial/error creation.
 * status=0 acknowledges creation only; caller must confirm ready before IO. */
MRAggregateGroup *mr_aggregate_group_create(const char *const *uids,
    const MRAggregateMemberInfo *members, uint32_t count,
    uint32_t primary, int32_t *status);
/* true consumes ownership; false retains ownership and requires caller retry. */
bool mr_aggregate_group_try_destroy(MRAggregateGroup *group);
bool mr_aggregate_group_ready(MRAggregateGroup *group);
typedef struct MRAggregateNativeBinding { uint32_t device, offset, total; } MRAggregateNativeBinding;
bool mr_aggregate_group_binding(MRAggregateGroup *group, const char *uid, bool input,
    uint32_t channels, MRAggregateNativeBinding *binding);
#endif
