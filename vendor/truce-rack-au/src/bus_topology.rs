//! Native bus discovery, channel grouping and topology validation.

use objc2_audio_toolbox::{
    AudioComponentInstance, AudioUnitElement, AudioUnitGetProperty, AudioUnitGetPropertyInfo,
    AudioUnitPropertyID, AudioUnitScope, kAudioUnitProperty_AudioChannelLayout,
    kAudioUnitProperty_ElementCount, kAudioUnitProperty_ElementName,
    kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, kAudioUnitScope_Output,
};
use objc2_core_audio_types::{
    AudioChannelDescription, AudioChannelLayout, AudioStreamBasicDescription,
    kAudioChannelLayoutTag_AudioUnit_5_1, kAudioChannelLayoutTag_AudioUnit_7_1,
    kAudioChannelLayoutTag_Mono, kAudioChannelLayoutTag_Stereo,
};
use objc2_core_foundation::CFString;
use std::ptr;
use truce_rack_core::bus::{Bus, BusKind, BusLayout, ChannelConfig};
use truce_rack_core::error::{Error, Result};

pub(super) fn channel_config(channels: u32, native_layout_tag: Option<u32>) -> ChannelConfig {
    match (channels, native_layout_tag) {
        (1, Some(tag)) if tag == kAudioChannelLayoutTag_Mono => ChannelConfig::Mono,
        (2, Some(tag)) if tag == kAudioChannelLayoutTag_Stereo => ChannelConfig::Stereo,
        (6, Some(tag)) if tag == kAudioChannelLayoutTag_AudioUnit_5_1 => ChannelConfig::Surround5_1,
        (8, Some(tag)) if tag == kAudioChannelLayoutTag_AudioUnit_7_1 => ChannelConfig::Surround7_1,
        (other, _) => ChannelConfig::Discrete(other),
    }
}

pub(super) fn selected_channel_config_matches(
    selected: ChannelConfig,
    channels: u32,
    native_layout_tag: Option<u32>,
) -> bool {
    let verified = channel_config(channels, native_layout_tag);
    selected == verified || selected == ChannelConfig::Discrete(channels)
}

pub(super) fn validate_bus_channels(inputs: &[u32], outputs: &[u32]) -> Result<()> {
    if outputs.is_empty() {
        return Err(Error::Other("Audio Unit topology has no output bus".into()));
    }
    for (direction, counts) in [("input", inputs), ("output", outputs)] {
        if counts.len() > 64 {
            return Err(Error::Other(format!(
                "Audio Unit topology exceeds 64 {direction} buses"
            )));
        }
        let mut total = 0_u32;
        for &count in counts {
            if count == 0 {
                return Err(Error::Other(format!(
                    "Audio Unit topology has a zero-channel {direction} bus"
                )));
            }
            total = total.checked_add(count).ok_or_else(|| {
                Error::Other(format!("Audio Unit {direction} channel count overflow"))
            })?;
        }
        if total > 64 {
            return Err(Error::Other(format!(
                "Audio Unit topology exceeds 64 {direction} channels"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub(super) struct BusObservation<'a> {
    pub(super) channels: &'a [u32],
    pub(super) names: &'a [Option<String>],
    pub(super) layout_tags: &'a [Option<u32>],
}

impl<'a> BusObservation<'a> {
    pub(super) const fn new(
        channels: &'a [u32],
        names: &'a [Option<String>],
        layout_tags: &'a [Option<u32>],
    ) -> Self {
        Self {
            channels,
            names,
            layout_tags,
        }
    }
}

pub(super) fn native_bus_layout(
    inputs: BusObservation<'_>,
    outputs: BusObservation<'_>,
) -> Result<BusLayout> {
    let BusObservation {
        channels: input_channels,
        names: input_names,
        layout_tags: input_layout_tags,
    } = inputs;
    let BusObservation {
        channels: output_channels,
        names: output_names,
        layout_tags: output_layout_tags,
    } = outputs;
    validate_bus_channels(input_channels, output_channels)?;
    let mut layout = BusLayout::new();
    layout.id = None;
    for (index, &channels) in input_channels.iter().enumerate() {
        let fallback = if index == 0 {
            "Input".to_owned()
        } else {
            format!("Input {}", index + 1)
        };
        let name = input_names
            .get(index)
            .and_then(Option::as_ref)
            .filter(|name| !name.trim().is_empty())
            .cloned()
            .unwrap_or(fallback);
        layout.inputs.push(Bus {
            name,
            kind: if index == 0 {
                BusKind::Main
            } else {
                BusKind::Auxiliary
            },
            channels: channel_config(channels, input_layout_tags.get(index).copied().flatten()),
        });
    }
    for (index, &channels) in output_channels.iter().enumerate() {
        let fallback = if index == 0 {
            "Output".to_owned()
        } else {
            format!("Output {}", index + 1)
        };
        let name = output_names
            .get(index)
            .and_then(Option::as_ref)
            .filter(|name| !name.trim().is_empty())
            .cloned()
            .unwrap_or(fallback);
        layout.outputs.push(Bus {
            name,
            kind: if index == 0 {
                BusKind::Main
            } else {
                BusKind::Auxiliary
            },
            channels: channel_config(channels, output_layout_tags.get(index).copied().flatten()),
        });
    }
    Ok(layout)
}

/// Read a fixed-width `UInt32` AU property.
///
/// # Safety
/// `unit` must be a live Audio Unit instance, and the caller must serialize this query with all
/// other native access to the instance.
pub(super) unsafe fn read_u32_property(
    unit: AudioComponentInstance,
    property: AudioUnitPropertyID,
    scope: AudioUnitScope,
    element: AudioUnitElement,
) -> Result<u32> {
    let mut value = 0_u32;
    let mut size = u32::try_from(std::mem::size_of::<u32>()).unwrap_or(0);
    let status = unsafe {
        AudioUnitGetProperty(
            unit,
            property,
            scope,
            element,
            ptr::NonNull::new((&raw mut value).cast())
                .ok_or_else(|| Error::Other("null Audio Unit property storage".into()))?,
            ptr::NonNull::new(&raw mut size)
                .ok_or_else(|| Error::Other("null Audio Unit property-size storage".into()))?,
        )
    };
    if status != 0 || usize::try_from(size).ok() != Some(std::mem::size_of::<u32>()) {
        return Err(Error::Other(format!(
            "AudioUnitGetProperty({property}, scope {scope}, element {element}) failed: OSStatus {status}, size {size}"
        )));
    }
    Ok(value)
}

/// Read one AU element's native stream format.
///
/// # Safety
/// `unit` must be a live Audio Unit instance, and the caller must serialize this query with all
/// other native access to the instance.
pub(super) unsafe fn read_stream_format(
    unit: AudioComponentInstance,
    scope: AudioUnitScope,
    element: AudioUnitElement,
) -> Result<AudioStreamBasicDescription> {
    let mut format = std::mem::MaybeUninit::<AudioStreamBasicDescription>::zeroed();
    let mut size = u32::try_from(std::mem::size_of::<AudioStreamBasicDescription>()).unwrap_or(0);
    let status = unsafe {
        AudioUnitGetProperty(
            unit,
            kAudioUnitProperty_StreamFormat,
            scope,
            element,
            ptr::NonNull::new(format.as_mut_ptr().cast())
                .ok_or_else(|| Error::Other("null Audio Unit stream-format storage".into()))?,
            ptr::NonNull::new(&raw mut size)
                .ok_or_else(|| Error::Other("null Audio Unit property-size storage".into()))?,
        )
    };
    if status != 0
        || usize::try_from(size).ok() != Some(std::mem::size_of::<AudioStreamBasicDescription>())
    {
        return Err(Error::Other(format!(
            "AudioUnitGetProperty(StreamFormat, scope {scope}, element {element}) failed: OSStatus {status}, size {size}"
        )));
    }
    // SAFETY: the AU succeeded and reported exactly the initialized ASBD size.
    Ok(unsafe { format.assume_init() })
}

/// Read the retained name for one AU element, when the property is implemented.
///
/// # Safety
/// `unit` must be a live Audio Unit instance, and the caller must serialize this query with all
/// other native access to the instance.
pub(super) unsafe fn read_element_name(
    unit: AudioComponentInstance,
    scope: AudioUnitScope,
    element: AudioUnitElement,
) -> Option<String> {
    use objc2_core_foundation::CFRetained;

    let mut raw_name: *const CFString = ptr::null();
    let mut size = u32::try_from(std::mem::size_of::<*const CFString>()).ok()?;
    let status = unsafe {
        AudioUnitGetProperty(
            unit,
            kAudioUnitProperty_ElementName,
            scope,
            element,
            ptr::NonNull::new((&raw mut raw_name).cast())?,
            ptr::NonNull::new(&raw mut size)?,
        )
    };
    if status != 0 || usize::try_from(size).ok() != Some(std::mem::size_of::<*const CFString>()) {
        return None;
    }
    let name_ptr = ptr::NonNull::new(raw_name.cast_mut())?;
    // SAFETY: AudioUnitProperties.h specifies that the host owns the returned CFStringRef.
    let name = unsafe { CFRetained::<CFString>::from_raw(name_ptr) };
    Some(name.to_string())
}

/// Read a recognized native channel-layout tag for an AU element.
///
/// Custom descriptions and unavailable/oversized layout properties remain unclassified, causing
/// the host-facing layout to use `Discrete(channel_count)` rather than guessing channel roles.
///
/// # Safety
/// `unit` must be a live Audio Unit instance, and the caller must serialize this query with all
/// other native access to the instance.
pub(super) unsafe fn read_channel_layout_tag(
    unit: AudioComponentInstance,
    scope: AudioUnitScope,
    element: AudioUnitElement,
) -> Option<u32> {
    let mut size = 0_u32;
    let info_status = unsafe {
        AudioUnitGetPropertyInfo(
            unit,
            kAudioUnitProperty_AudioChannelLayout,
            scope,
            element,
            &raw mut size,
            ptr::null_mut(),
        )
    };
    let header_size = std::mem::offset_of!(AudioChannelLayout, mChannelDescriptions);
    let max_size = header_size
        .checked_add(64_usize.checked_mul(std::mem::size_of::<AudioChannelDescription>())?)?;
    let property_size = usize::try_from(size).ok()?;
    if info_status != 0 || property_size < header_size || property_size > max_size {
        return None;
    }

    // Use u64 backing words to keep the variable-length structure suitably aligned. This runs
    // during topology discovery/activation, never from the render callback.
    let word_count =
        property_size.checked_add(std::mem::size_of::<u64>() - 1)? / std::mem::size_of::<u64>();
    let mut storage = vec![0_u64; word_count];
    let mut io_size = size;
    let status = unsafe {
        AudioUnitGetProperty(
            unit,
            kAudioUnitProperty_AudioChannelLayout,
            scope,
            element,
            ptr::NonNull::new(storage.as_mut_ptr().cast())?,
            ptr::NonNull::new(&raw mut io_size)?,
        )
    };
    let returned_size = usize::try_from(io_size).ok()?;
    if status != 0 || returned_size < header_size || returned_size > property_size {
        return None;
    }

    let layout_ptr = storage.as_ptr().cast::<AudioChannelLayout>();
    // SAFETY: property retrieval wrote a full layout header into aligned storage, as bounded
    // above; both fields are plain integer values at the documented offsets.
    let tag = unsafe { ptr::read_unaligned(ptr::addr_of!((*layout_ptr).mChannelLayoutTag)) };
    let description_count =
        unsafe { ptr::read_unaligned(ptr::addr_of!((*layout_ptr).mNumberChannelDescriptions)) };
    (description_count == 0).then_some(tag)
}

/// Observe the AU's current per-element input and output topology.
///
/// # Safety
/// `unit` must be a live Audio Unit instance, and the caller must serialize these queries with
/// all other native access to the instance.
pub(super) unsafe fn discover_bus_layout(
    unit: AudioComponentInstance,
) -> Result<(BusLayout, Vec<u32>, Vec<u32>)> {
    let input_count = unsafe {
        read_u32_property(
            unit,
            kAudioUnitProperty_ElementCount,
            kAudioUnitScope_Input,
            0,
        )?
    };
    let output_count = unsafe {
        read_u32_property(
            unit,
            kAudioUnitProperty_ElementCount,
            kAudioUnitScope_Output,
            0,
        )?
    };
    let input_count = usize::try_from(input_count)
        .ok()
        .filter(|&count| count <= 64)
        .ok_or_else(|| Error::Other("Audio Unit input bus count exceeds 64".into()))?;
    let output_count = usize::try_from(output_count)
        .ok()
        .filter(|&count| count <= 64)
        .ok_or_else(|| Error::Other("Audio Unit output bus count exceeds 64".into()))?;
    let mut input_channels = Vec::with_capacity(input_count);
    let mut input_names = Vec::with_capacity(input_count);
    let mut input_layout_tags = Vec::with_capacity(input_count);
    for index in 0..input_count {
        let element = u32::try_from(index)
            .map_err(|_| Error::Other("Audio Unit input bus index overflow".into()))?;
        let format = unsafe { read_stream_format(unit, kAudioUnitScope_Input, element)? };
        input_channels.push(format.mChannelsPerFrame);
        input_names.push(unsafe { read_element_name(unit, kAudioUnitScope_Input, element) });
        input_layout_tags
            .push(unsafe { read_channel_layout_tag(unit, kAudioUnitScope_Input, element) });
    }
    let mut output_channels = Vec::with_capacity(output_count);
    let mut output_names = Vec::with_capacity(output_count);
    let mut output_layout_tags = Vec::with_capacity(output_count);
    for index in 0..output_count {
        let element = u32::try_from(index)
            .map_err(|_| Error::Other("Audio Unit output bus index overflow".into()))?;
        let format = unsafe { read_stream_format(unit, kAudioUnitScope_Output, element)? };
        output_channels.push(format.mChannelsPerFrame);
        output_names.push(unsafe { read_element_name(unit, kAudioUnitScope_Output, element) });
        output_layout_tags
            .push(unsafe { read_channel_layout_tag(unit, kAudioUnitScope_Output, element) });
    }
    let layout = native_bus_layout(
        BusObservation::new(&input_channels, &input_names, &input_layout_tags),
        BusObservation::new(&output_channels, &output_names, &output_layout_tags),
    )?;
    Ok((layout, input_channels, output_channels))
}
