//! Running applications that can be captured, from the native application enumerator.
use super::capture_error::{CaptureOperation, CaptureProbeError, NATIVE_OK};
use mixrelay_audio_graph::ApplicationSelector;
use mixrelay_audio_io::CapturableApplication;
use mixrelay_macos_process_tap::TapError;
use std::ffi::{CStr, c_char, c_void};

type ApplicationVisitor =
    unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *const c_char, i32);

// Compiled into the process-tap crate from the desktop's Objective-C bridge.
unsafe extern "C" {
    fn mr_process_tap_visit_applications(
        visitor: Option<ApplicationVisitor>,
        context: *mut c_void,
    ) -> TapError;
}

/// One entry per app, ordered by display name.
pub(super) fn capturable_applications() -> Result<Vec<CapturableApplication>, CaptureProbeError> {
    let mut applications: Vec<CapturableApplication> = Vec::new();
    // SAFETY: enumeration and callbacks are synchronous and `applications` outlives the call.
    let error = unsafe {
        mr_process_tap_visit_applications(Some(collect_application), (&raw mut applications).cast())
    };
    if error.code != NATIVE_OK {
        return Err(CaptureProbeError::new(CaptureOperation::Enumerate, error));
    }
    Ok(one_per_application(applications))
}

unsafe extern "C" fn collect_application(
    context: *mut c_void,
    bundle_id: *const c_char,
    display_name: *const c_char,
    icon_data_url: *const c_char,
    process_id: i32,
) {
    if context.is_null() || bundle_id.is_null() || display_name.is_null() {
        return;
    }
    // SAFETY: the native enumerator keeps every pointer valid for this synchronous
    // callback; strings are NUL-terminated and the icon is optional.
    let (applications, bundle_id, display_name, icon_data_url) = unsafe {
        (
            &mut *context.cast::<Vec<CapturableApplication>>(),
            CStr::from_ptr(bundle_id),
            CStr::from_ptr(display_name),
            (!icon_data_url.is_null()).then(|| CStr::from_ptr(icon_data_url)),
        )
    };
    applications.extend(capturable_application(
        bundle_id,
        display_name,
        icon_data_url,
        process_id,
    ));
}

/// An enumerated process the user can select; `None` without a valid bundle ID, display
/// name and process ID.
fn capturable_application(
    bundle_id: &CStr,
    display_name: &CStr,
    icon_data_url: Option<&CStr>,
    process_id: i32,
) -> Option<CapturableApplication> {
    let bundle_id = bundle_id.to_str().ok().filter(|id| !id.is_empty())?;
    let display_name = display_name.to_str().ok().filter(|name| !name.is_empty())?;
    let process_id = u32::try_from(process_id).ok()?;
    let icon_data_url = icon_data_url
        .and_then(|icon| icon.to_str().ok())
        .filter(|icon| !icon.is_empty())
        .map(str::to_owned);
    Some(CapturableApplication {
        bundle_id: bundle_id.to_owned(),
        display_name: display_name.to_owned(),
        icon_data_url,
        process_id,
        selector: ApplicationSelector::BundleId {
            bundle_id: bundle_id.to_owned(),
            include_descendants: false,
        },
    })
}

/// Keeps each bundle's lowest process ID, ordered by display name, then bundle ID.
fn one_per_application(mut applications: Vec<CapturableApplication>) -> Vec<CapturableApplication> {
    applications.sort_unstable_by(|left, right| {
        left.bundle_id
            .cmp(&right.bundle_id)
            .then(left.process_id.cmp(&right.process_id))
    });
    applications.dedup_by(|right, left| right.bundle_id == left.bundle_id);
    applications.sort_unstable_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then(left.bundle_id.cmp(&right.bundle_id))
    });
    applications
}

#[cfg(test)]
mod tests {
    use super::*;

    fn application(bundle_id: &str, display_name: &str, process_id: i32) -> CapturableApplication {
        let bundle = std::ffi::CString::new(bundle_id).unwrap();
        let name = std::ffi::CString::new(display_name).unwrap();
        capturable_application(&bundle, &name, Some(c""), process_id).unwrap()
    }

    #[test]
    fn each_app_is_listed_once_under_its_lowest_process_id_in_display_order() {
        let listed = one_per_application(vec![
            application("org.example.Zed", "Zed", 40),
            application("org.example.Browser", "Browser", 31),
            application("org.example.Browser", "Browser", 12),
            application("org.example.Another", "Browser", 50),
        ]);
        let order: Vec<_> = listed
            .iter()
            .map(|app| (app.bundle_id.as_str(), app.process_id))
            .collect();
        assert_eq!(
            order,
            [
                ("org.example.Another", 50),
                ("org.example.Browser", 12),
                ("org.example.Zed", 40),
            ]
        );
        assert_eq!(listed[0].icon_data_url, None, "an empty icon is no icon");
    }

    #[test]
    fn processes_without_a_usable_identity_are_not_selectable() {
        assert!(capturable_application(c"", c"Name", None, 1).is_none());
        assert!(capturable_application(c"org.example.App", c"", None, 1).is_none());
        assert!(capturable_application(c"org.example.App", c"Name", None, -1).is_none());
        assert!(capturable_application(c"\xff", c"Name", None, 1).is_none());
        let app = capturable_application(c"org.example.App", c"Name", Some(c"data:"), 7).unwrap();
        assert_eq!(app.icon_data_url.as_deref(), Some("data:"));
        assert_eq!(
            app.selector,
            ApplicationSelector::BundleId {
                bundle_id: "org.example.App".into(),
                include_descendants: false,
            }
        );
    }
}
