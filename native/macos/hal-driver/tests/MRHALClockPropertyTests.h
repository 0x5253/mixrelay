/* The SDK host reads these properties before IO and reentrantly from callbacks. */
static void assert_clock_properties(AudioObjectID device) {
    const AudioObjectPropertySelector selectors[] = {
        kAudioDevicePropertyClockAlgorithm,
        kAudioDevicePropertyZeroTimeStampPeriod,
        kAudioDevicePropertyClockIsStable,
    };
    const UInt32 expected[] = {kAudioDeviceClockAlgorithmRaw, 16384, 1};
    for (size_t index = 0; index < sizeof(selectors) / sizeof(selectors[0]); ++index) {
        const AudioObjectPropertyAddress property = {selectors[index], kAudioObjectPropertyScopeGlobal, 0};
        assert((*gDriver)->HasProperty(gDriver, device, getpid(), &property));
        UInt32 size = 0, written = 0, value = 0;
        assert((*gDriver)->GetPropertyDataSize(gDriver, device, getpid(), &property, 0, NULL, &size) == noErr);
        assert(size == sizeof(value));
        assert((*gDriver)->GetPropertyData(gDriver, device, getpid(), &property,
            0, NULL, sizeof(value), &written, &value) == noErr);
        assert(written == sizeof(value) && value == expected[index]);
        Boolean writable = true;
        assert((*gDriver)->IsPropertySettable(gDriver, device, getpid(), &property, &writable) == noErr);
        assert(!writable);
        assert((*gDriver)->SetPropertyData(gDriver, device, getpid(), &property,
            0, NULL, sizeof(value), &value) == kAudioHardwareIllegalOperationError);
        assert((*gDriver)->GetPropertyData(gDriver, device, getpid(), &property,
            0, NULL, sizeof(value) - 1, &written, &value) == kAudioHardwareBadPropertySizeError);
        assert(!(*gDriver)->HasProperty(gDriver, device + 1, getpid(), &property));
    }
}
