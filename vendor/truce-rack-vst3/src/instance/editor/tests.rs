use super::*;

#[test]
fn native_view_dimensions_reject_signed_overflow_and_inversion() {
    assert_eq!(
        view_size(ViewRect {
            left: 10,
            top: 20,
            right: 650,
            bottom: 500
        }),
        Some((640, 480))
    );
    assert_eq!(
        view_size(ViewRect {
            left: i32::MIN,
            top: 0,
            right: i32::MAX,
            bottom: 500
        }),
        None
    );
    assert_eq!(
        view_size(ViewRect {
            left: 0,
            top: i32::MIN,
            right: 650,
            bottom: i32::MAX
        }),
        None
    );
    assert_eq!(
        view_size(ViewRect {
            left: 10,
            top: 20,
            right: 9,
            bottom: 500
        }),
        None
    );
}
