#ifndef MR_REMOVAL_SESSION_H
#define MR_REMOVAL_SESSION_H
#include <stdint.h>

enum MRRemovalEvent {
    MRRemovalAdmission = 1,
    MRRemovalTransaction = 2,
    MRRemovalAcknowledged = 3,
    MRRemovalDisconnected = 4,
    MRRemovalAbandoned = 5,
    MRRemovalIdleDeadline = 6,
    MRRemovalTerminated = 7,
};
enum MRRemovalAction {
    MRRemovalIgnore = 0,
    MRRemovalAllowRemoval = 1,
    MRRemovalAwaitAcknowledgement = 2,
    MRRemovalFinish = 4,
    MRRemovalIdle = 8,
    MRRemovalAwaitIdle = 16,
};
#endif
