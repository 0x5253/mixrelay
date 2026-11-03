use super::*;

#[test]
fn queued_callbacks_cannot_adopt_a_retired_service_owner() {
    let service = crate::tests::service();
    let context = Box::new(Arc::downgrade(&service));
    let pointer = ptr::from_ref(context.as_ref()).cast_mut().cast();
    // SAFETY: this test owns and retains the boxed Weak throughout both reads.
    assert!(unsafe { service_context(pointer) }.is_some());
    drop(service);
    // SAFETY: callbacks receive the same live context after its target retired.
    assert!(unsafe { service_context(pointer) }.is_none());
    assert!(!unsafe { mr_driver_service_accept_peer(pointer, ptr::null_mut()) });
    // SAFETY: a retired owner rejects callbacks before any OS/native observation.
    unsafe {
        mr_driver_service_peer_event(pointer, ptr::null_mut(), ptr::null_mut());
        mr_driver_service_control_tick(pointer);
    }
}
