//! Initialize component and controller with rollback guards.

use crate::{host_context, lifecycle};
use std::ptr;
use truce_rack_core::{
    error::{Error, Result},
    info::PluginInfo,
};
use vst3::{
    ComPtr,
    Steinberg::{
        IPluginBase, IPluginFactory, IPluginFactoryTrait, TUID,
        Vst::{IAudioProcessor, IComponent, IComponentTrait, IEditController},
        kResultOk, kResultTrue,
    },
};

pub(super) struct LoadContext<'a> {
    pub(super) info: &'a PluginInfo,
    pub(super) factory: &'a ComPtr<IPluginFactory>,
    pub(super) host_services: &'a host_context::HostServices,
}

pub(super) struct ComponentParts {
    pub(super) component: ComPtr<IComponent>,
    pub(super) processor: ComPtr<IAudioProcessor>,
    pub(super) lifecycle: lifecycle::InitializedPlugin,
}

pub(super) struct ControllerParts {
    pub(super) controller: ComPtr<IEditController>,
    pub(super) separate: bool,
    pub(super) lifecycle: Option<lifecycle::InitializedPlugin>,
}

pub(super) fn load_component(context: &LoadContext<'_>, cid: &TUID) -> Result<ComponentParts> {
    let component =
        unsafe { create_instance::<IComponent>(context.factory, cid) }.ok_or_else(|| {
            Error::LoadFailed {
                path: context.info.path.clone(),
                reason: "factory.createInstance(IComponent) returned NULL".into(),
            }
        })?;
    let base = component
        .as_com_ref()
        .cast::<IPluginBase>()
        .ok_or_else(|| Error::LoadFailed {
            path: context.info.path.clone(),
            reason: "component lacks mandatory IPluginBase".into(),
        })?;
    let lifecycle =
        lifecycle::InitializedPlugin::initialize(&base, context.host_services.context_ptr())
            .map_err(|error| load_error(context.info, &error))?;
    let processor = component
        .as_com_ref()
        .cast::<IAudioProcessor>()
        .ok_or_else(|| Error::LoadFailed {
            path: context.info.path.clone(),
            reason: "component does not implement IAudioProcessor".into(),
        })?;
    Ok(ComponentParts {
        component,
        processor,
        lifecycle,
    })
}

pub(super) fn load_controller(
    context: &LoadContext<'_>,
    component: &ComPtr<IComponent>,
) -> Result<ControllerParts> {
    // A combined component exposes its controller directly and need not return a class ID.
    // Query it first, matching the SDK host's PlugProvider initialization sequence.
    if let Some(controller) = component.as_com_ref().cast::<IEditController>() {
        return Ok(ControllerParts {
            controller,
            separate: false,
            lifecycle: None,
        });
    }
    let mut cid: TUID = [0; 16];
    let status = unsafe { component.getControllerClassId(&raw mut cid) };
    if status == kResultTrue {
        let controller = unsafe { create_instance::<IEditController>(context.factory, &cid) }
            .ok_or_else(|| Error::LoadFailed {
                path: context.info.path.clone(),
                reason: "factory.createInstance(IEditController) returned NULL".into(),
            })?;
        let base = controller
            .as_com_ref()
            .cast::<IPluginBase>()
            .ok_or_else(|| Error::LoadFailed {
                path: context.info.path.clone(),
                reason: "controller lacks mandatory IPluginBase".into(),
            })?;
        let lifecycle = context
            .host_services
            .initialize_separate_controller(&base, &controller)
            .map_err(|error| load_error(context.info, &error))?;
        return Ok(ControllerParts {
            controller,
            separate: true,
            lifecycle: Some(lifecycle),
        });
    }
    if status != vst3::Steinberg::kResultFalse {
        return Err(Error::LoadFailed {
            path: context.info.path.clone(),
            reason: format!("IComponent::getControllerClassId failed ({status})"),
        });
    }
    Err(Error::LoadFailed {
        path: context.info.path.clone(),
        reason: "component reported no separate controller but lacks IEditController".into(),
    })
}

fn load_error(info: &PluginInfo, error: &Error) -> Error {
    Error::LoadFailed {
        path: info.path.clone(),
        reason: error.to_string(),
    }
}

/// Run `factory.createInstance` for `I` and wrap the result as a
/// `ComPtr<I>`. The factory call uses the interface's `IID`
/// directly so the plugin returns the correct `*mut c_void`.
unsafe fn create_instance<I>(factory: &ComPtr<IPluginFactory>, cid: &TUID) -> Option<ComPtr<I>>
where
    I: vst3::Interface,
{
    let mut obj: *mut std::ffi::c_void = ptr::null_mut();
    // `com_scrape_types::Guid` and `TUID` share the same
    // 16-byte layout — reinterpret to avoid an extra copy, independent of c_char signedness.
    let iid_bytes: &TUID =
        unsafe { &*(std::ptr::from_ref::<vst3::com_scrape_types::Guid>(&I::IID).cast::<TUID>()) };
    let cid_ptr = cid.as_ptr();
    let iid_ptr = iid_bytes.as_ptr();
    if unsafe { factory.createInstance(cid_ptr, iid_ptr, &raw mut obj) } != kResultOk
        || obj.is_null()
    {
        return None;
    }
    unsafe { ComPtr::<I>::from_raw(obj.cast()) }
}
