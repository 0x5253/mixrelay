/* Lab-only independent RTP sample clock. Sends synthetic stereo L16 exclusively
 * over localhost. Absolute CLOCK_MONOTONIC deadlines inject a measured clock skew;
 * packet timestamps count samples and never follow PipeWire callback timing. */
#define _POSIX_C_SOURCE 200809L
#include <arpa/inet.h>
#include <errno.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

static uint64_t now_ns(void) {
    struct timespec value;
    clock_gettime(CLOCK_MONOTONIC, &value);
    return (uint64_t)value.tv_sec * 1000000000u + (uint64_t)value.tv_nsec;
}
static void put16(uint8_t *p, uint16_t value) {
    p[0] = value >> 8; p[1] = value & 255;
}
static void put32(uint8_t *p, uint32_t value) {
    p[0] = value >> 24; p[1] = value >> 16; p[2] = value >> 8; p[3] = value;
}
int main(int argc, char **argv) {
    if (argc != 4) return 2;
    int port = atoi(argv[1]);
    double ppm = strtod(argv[2], NULL), seconds = strtod(argv[3], NULL);
    if (port < 1024 || port > 65535 || !isfinite(ppm) || fabs(ppm) > 10000 || seconds < 1 || seconds > 600) return 2;
    int fd = socket(AF_INET, SOCK_DGRAM, 0);
    if (fd < 0) return 1;
    struct sockaddr_in address = { .sin_family = AF_INET, .sin_port = htons(port) };
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    /* PipeWire 1.4's unicast RTP listener filters for this source port. */
    int reuse = 1;
    if (setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof(reuse)) < 0 ||
        bind(fd, (struct sockaddr *)&address, sizeof(address)) < 0) {
        close(fd);
        return 1;
    }
    uint8_t packet[12 + 240 * 4] = { 0x80, 96 };
    put32(packet + 8, 0x4d524452);
    double rate = 48000.0 * (1.0 + ppm / 1000000.0);
    uint64_t start = now_ns(), first = 0, last = 0, packets = 0, max_late = 0;
    long double late_square = 0;
    uint64_t total_packets = (uint64_t)(seconds * rate / 240.0);
    for (uint64_t index = 0; index < total_packets; index++) {
        uint64_t deadline = start + (uint64_t)((long double)(index * 240) * 1e9L / rate);
        struct timespec when = { .tv_sec = deadline / 1000000000, .tv_nsec = deadline % 1000000000 };
        int result;
        do { result = clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &when, NULL); } while (result == EINTR);
        if (result) return 1;
        put16(packet + 2, (uint16_t)index);
        put32(packet + 4, (uint32_t)(index * 240));
        for (uint32_t frame = 0; frame < 240; frame++) {
            double phase = 6.283185307179586 * (double)((index * 240 + frame) % 48) / 48.0;
            int16_t sample = (int16_t)lrint(8192.0 * sin(phase));
            put16(packet + 12 + frame * 4, (uint16_t)sample);
            put16(packet + 14 + frame * 4, (uint16_t)sample);
        }
        uint64_t sent = now_ns();
        if (sendto(fd, packet, sizeof(packet), 0, (struct sockaddr *)&address, sizeof(address)) != sizeof(packet)) return 1;
        if (!packets) first = sent;
        last = sent; packets++;
        uint64_t late = sent > deadline ? sent - deadline : 0;
        if (late > max_late) max_late = late;
        late_square += (long double)late * late;
    }
    close(fd);
    double observed = packets > 1 ? (double)((packets - 1) * 240) * 1e9 / (double)(last - first) : 0;
    printf("{\"requestedPpm\":%.3f,\"packets\":%llu,\"frames\":%llu,\"firstSendNs\":%llu,\"lastSendNs\":%llu,\"observedRate\":%.6f,\"observedPpm\":%.3f,\"maxLatenessUs\":%.3f,\"rmsLatenessUs\":%.3f}\n",
        ppm, (unsigned long long)packets, (unsigned long long)(packets * 240),
        (unsigned long long)first, (unsigned long long)last, observed, (observed / 48000 - 1) * 1e6,
        max_late / 1000.0, sqrt((double)(late_square / packets)) / 1000.0);
    return 0;
}
