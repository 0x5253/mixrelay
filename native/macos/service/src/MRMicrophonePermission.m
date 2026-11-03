#import <AVFoundation/AVFoundation.h>
#import <Security/AuthSession.h>

// Return the public API's status without requesting consent from a background session.
long mr_microphone_authorization_status(void) {
    SessionAttributeBits attributes = 0;
    if (SessionGetInfo(callerSecuritySession, NULL, &attributes) == errSecSuccess &&
        !(attributes & sessionHasGraphicAccess)) {
        // AVFoundation reports denial in a daemon session even when the user's
        // CoreAudio broker can request consent. The device registration owns it.
        return 4;
    }
    return (long)[AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
}

// Request asynchronously; Rust observes the decision with a bounded control-side wait.
void mr_microphone_request_authorization(void) {
    [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted) {
        (void)granted;
    }];
}
