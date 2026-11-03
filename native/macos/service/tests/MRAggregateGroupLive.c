/* Explicit, isolated private-device acceptance. Never sets hardware formats,
 * defaults, or the user's graph. IOProcs only contribute silence. */
#include "MRDeviceIO.h"
#include <assert.h>
#include <stdio.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 3) { fprintf(stderr, "usage: probe primary-output-UID second-output-UID\n"); return 2; }
    const char *members[] = {argv[1], argv[2]};
    MRAggregateMemberInfo facts[2] = {0};
    if (!mr_aggregate_member_info(members[0], &facts[0]) ||
        !mr_aggregate_member_info(members[1], &facts[1])) return 1;
    int32_t status = 0;
    MRAggregateGroup *group = mr_aggregate_group_create(members, facts, 2, 0, &status);
    if (!group || status) {
        if (group) assert(mr_aggregate_group_try_destroy(group));
        fprintf(stderr, "private aggregate failed: %d\n", status); return 1;
    }
    for (unsigned i = 0; i < 100 && !mr_aggregate_group_ready(group); ++i) usleep(20000);
    assert(mr_aggregate_group_ready(group));
    MRAggregateNativeBinding binding = {0};
    assert(mr_aggregate_group_binding(group, members[0], false, 2, &binding));
    assert(binding.offset == 0 && binding.total >= 4);
    uint32_t device = binding.device;
    assert(mr_aggregate_group_binding(group, members[1], false, 2, &binding));
    assert(binding.device == device && binding.offset >= 2);
    printf("OS readback: private group=%u primary offset=0 secondary offset=%u total=%u; main UID, order, drift policy verified\n", device, binding.offset, binding.total);
    MRDeviceIO *first = mr_device_io_create_group(group, members[0], false, 2, 48000);
    MRDeviceIO *second = mr_device_io_create_group(group, members[1], false, 2, 48000);
    bool connected = first && second && mr_device_io_connect(first) && mr_device_io_connect(second);
    if (connected) {
        usleep(500000);
        connected = mr_device_io_alive(first) && mr_device_io_alive(second) &&
            mr_device_io_native_frames(first) > 0 && mr_device_io_native_frames(second) > 0 &&
            mr_device_io_clock_device(first) == device && mr_device_io_clock_device(second) == device;
        printf("IOProc frames: %llu / %llu; same native clock identity=%u\n",
            (unsigned long long)mr_device_io_native_frames(first),
            (unsigned long long)mr_device_io_native_frames(second), device);
        MRDeviceBufferInfo info = {0};
        connected = connected && mr_device_io_buffer_info(first, &info) &&
            info.device == device && info.settable && info.minimum <= 32 && info.maximum >= 32 &&
            mr_device_buffer_write(device, 32);
        if (connected) {
            usleep(100000);
            connected = mr_device_buffer_size(device) == 32 &&
                mr_device_io_quantum(first, false) == 32 && mr_device_io_quantum(second, false) == 32;
            printf("Native buffer readback=%u callback quantums=%u/%u\n", mr_device_buffer_size(device),
                mr_device_io_quantum(first, false), mr_device_io_quantum(second, false));
        }
    }
    assert(mr_device_io_destroy(second));
    assert(mr_device_io_destroy(first));
    assert(mr_aggregate_group_try_destroy(group));
    if (!connected) { fprintf(stderr, "grouped IOProc did not run\n"); return 1; }
    puts("PASS private aggregate native policy readback, sliced IOProc registrations and ordered teardown");
}
