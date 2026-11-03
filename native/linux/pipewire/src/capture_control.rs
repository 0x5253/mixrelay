//! Control-only preparation of one owned RAW capture adapter before activation.
use std::{
    io::Cursor,
    sync::{Arc, Mutex},
};

use pipewire::{
    node::{Node, NodeListener},
    proxy::ProxyT,
    registry::{GlobalObject, RegistryBox},
    spa::{
        param::ParamType,
        pod::{
            ChoiceValue, Object, Pod, Property, Value, deserialize::PodDeserializer,
            serialize::PodSerializer,
        },
        utils::{
            ChoiceEnum, Id,
            result::{SpaResult, SpaSuccess},
        },
    },
};

use crate::Connection;
mod negotiation;
use negotiation::{CaptureHost, PreparationError, Snapshot};

#[derive(Default)]
struct Observation {
    serial: Option<u32>,
    generation: u64,
    sequence: Option<i32>,
    last_sequence: Option<i32>,
    node: Option<(u32, i32, u32, u32)>,
    reply: Vec<Vec<u8>>,
    error: Option<String>,
}

impl Observation {
    fn serial(&mut self, flags: u32) {
        if self.serial != Some(flags) {
            self.serial = Some(flags);
            self.generation = self.generation.wrapping_add(1);
        }
    }

    fn param(&mut self, sequence: i32, id: ParamType, pod: Option<&Pod>) {
        self.last_sequence = Some(sequence);
        if self.sequence != Some(sequence) || id != ParamType::PortConfig {
            return;
        }
        if let Some(pod) = pod {
            // Only owned bytes cross the control-loop callback boundary. SPA
            // Value may contain non-Send pointer variants; decode off callback.
            self.reply.push(pod.as_bytes().to_vec());
        }
    }
}

fn input_readback(replies: &[Vec<u8>]) -> Option<Object> {
    replies.iter().find_map(|bytes| {
        let Ok((_, Value::Object(object))) = PodDeserializer::deserialize_any_from(bytes) else {
            return None;
        };
        configured_input(&object).then_some(object)
    })
}

fn node_listener(node: &Node, observation: &Arc<Mutex<Observation>>) -> NodeListener {
    let info_observation = Arc::clone(observation);
    let param_observation = Arc::clone(observation);
    node.add_listener_local()
        .info(move |info| {
            let mut state = info_observation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.node = Some((
                info.id(),
                info.as_raw().state,
                info.n_input_ports(),
                info.n_output_ports(),
            ));
            for param in info.params() {
                if param.id() == ParamType::PortConfig {
                    state.serial(param.flags().bits());
                }
            }
        })
        .param(move |sequence, id, _, _, pod| {
            param_observation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .param(sequence, id, pod);
        })
        .register()
}

fn configured_input(object: &Object) -> bool {
    use pipewire::spa::sys;
    object.type_ == sys::SPA_TYPE_OBJECT_ParamPortConfig
        && input_direction(property(object, sys::SPA_PARAM_PORT_CONFIG_direction))
        && property(object, sys::SPA_PARAM_PORT_CONFIG_mode)
            == Some(&Value::Id(Id(sys::SPA_PARAM_PORT_CONFIG_MODE_dsp)))
        && matches!(
            property(object, sys::SPA_PARAM_PORT_CONFIG_format),
            Some(Value::Object(_))
        )
}

fn input_direction(value: Option<&Value>) -> bool {
    let input = Id(pipewire::spa::sys::SPA_DIRECTION_INPUT);
    match value {
        Some(Value::Id(direction)) => *direction == input,
        Some(Value::Choice(ChoiceValue::Id(choice))) => {
            matches!(&choice.1, ChoiceEnum::None(direction) if *direction == input)
        }
        _ => false,
    }
}

fn property(object: &Object, key: u32) -> Option<&Value> {
    object
        .properties
        .iter()
        .find(|property| property.key == key)
        .map(|property| &property.value)
}

fn monitor_config(mut object: Object) -> Object {
    let key = pipewire::spa::sys::SPA_PARAM_PORT_CONFIG_monitor;
    if let Some(property) = object
        .properties
        .iter_mut()
        .find(|property| property.key == key)
    {
        property.value = Value::Bool(true);
    } else {
        object
            .properties
            .push(Property::new(key, Value::Bool(true)));
    }
    object
}

fn writable_config(mut object: Object, rate: u32) -> Result<Object, String> {
    use pipewire::spa::sys;
    let format = object
        .properties
        .iter_mut()
        .find(|property| property.key == sys::SPA_PARAM_PORT_CONFIG_format)
        .ok_or("capture DSP PortConfig lacks format")?;
    let Value::Object(format) = &mut format.value else {
        return Err("capture DSP PortConfig format is not an object".into());
    };
    let rate = i32::try_from(rate).map_err(|_| "capture rate does not fit SPA format")?;
    let property = format
        .properties
        .iter_mut()
        .find(|property| property.key == sys::SPA_FORMAT_AUDIO_rate);
    // DSP readback normalizes rate to zero (the graph clock decides it), but
    // the PortConfig setter requires a positive initial format rate. This does
    // not change the RAW follower or add a second sample-rate converter.
    match property {
        Some(property) if property.value == Value::Int(0) => property.value = Value::Int(rate),
        Some(_) => {}
        None => format
            .properties
            .push(Property::new(sys::SPA_FORMAT_AUDIO_rate, Value::Int(rate))),
    }
    Ok(object)
}

struct Preparation<'a> {
    connection: &'a Connection,
    listener: Option<NodeListener>,
    core_listener: Option<pipewire::core::Listener>,
    registry_listener: Option<pipewire::registry::Listener>,
    node: Option<Node>,
    registry: Option<RegistryBox<'a>>,
    observation: Arc<Mutex<Observation>>,
}

impl Drop for Preparation<'_> {
    fn drop(&mut self) {
        let _guard = self.connection.lock();
        // Removing listeners under the loop lock drains their control callbacks
        // before the node/registry proxies or shared observation can disappear.
        self.listener.take();
        self.core_listener.take();
        self.registry_listener.take();
        self.node.take();
        self.registry.take();
    }
}

impl<'a> Preparation<'a> {
    fn new(connection: &'a Connection, id: u32) -> Result<Self, String> {
        let _guard = connection.lock();
        let registry = connection
            .core()
            .get_registry()
            .map_err(|error| error.to_string())?;
        // The caller pins the exact stream allocation through preparation. Its
        // exported ID cannot be recycled while that native owner remains live.
        let object: GlobalObject<&pipewire::spa::utils::dict::DictRef> = GlobalObject {
            id,
            permissions: pipewire::permissions::PermissionFlags::empty(),
            type_: pipewire::types::ObjectType::Node,
            version: pipewire::sys::PW_VERSION_NODE,
            props: None,
        };
        let node: Node = registry.bind(&object).map_err(|error| error.to_string())?;
        let observation = Arc::new(Mutex::new(Observation::default()));
        let removed = Arc::clone(&observation);
        let registry_listener = registry
            .add_listener_local()
            .global_remove(move |removed_id| {
                if removed_id == id {
                    removed
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .error =
                        Some("owned capture adapter was removed during preparation".into());
                }
            })
            .register();
        let listener = node_listener(&node, &observation);
        let errors = Arc::clone(&observation);
        let proxy_id = node.upcast_ref().id();
        let core_listener = connection
            .core()
            .add_listener_local()
            .error(move |id, _, result, message| {
                if id == proxy_id {
                    errors
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .error = Some(format!("capture PortConfig failed: {result}: {message}"));
                }
            })
            .register();
        Ok(Self {
            connection,
            listener: Some(listener),
            core_listener: Some(core_listener),
            registry_listener: Some(registry_listener),
            node: Some(node),
            registry: Some(registry),
            observation,
        })
    }

    fn node(&self) -> Result<&Node, String> {
        self.node
            .as_ref()
            .ok_or_else(|| "capture preparation closed".into())
    }

    fn request(&self) -> Result<(), String> {
        let _guard = self.connection.lock();
        let node = self.node()?;
        // SAFETY: the core and node proxy are retained by this preparation and
        // the control-loop lock excludes removal while resolving the proxy.
        let proxy = unsafe {
            pipewire::sys::pw_core_find_proxy(
                self.connection.core().as_raw_ptr(),
                node.upcast_ref().id(),
            )
        };
        if proxy.is_null() {
            return Err("capture node proxy was removed".into());
        }
        // SAFETY: this preparation owns the proxy; the loop lock prevents its
        // callbacks/teardown from racing the call and the returned sequence is
        // installed before callbacks resume. No pointer escapes this call.
        let result = unsafe {
            pipewire::spa::spa_interface_call_method!(
                proxy,
                pipewire::sys::pw_node_methods,
                enum_params,
                0,
                ParamType::PortConfig.as_raw(),
                0,
                u32::MAX,
                std::ptr::null()
            )
        };
        let sequence = match SpaResult::from_c(result)
            .into_result()
            .map_err(|error| error.to_string())?
        {
            SpaSuccess::Async(sequence) => sequence.raw(),
            SpaSuccess::Sync(_) => {
                return Err("capture enum_params lacked an async sequence".into());
            }
        };
        let mut state = self
            .observation
            .lock()
            .map_err(|_| "capture preparation poisoned")?;
        state.sequence = Some(sequence);
        state.reply.clear();
        Ok(())
    }

    fn write_monitor(&self, config: Object, rate: u32) -> Result<(Object, u64), String> {
        let wanted = monitor_config(config);
        let writable = writable_config(wanted.clone(), rate)?;
        let bytes = PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(writable))
            .map_err(|_| "capture PortConfig serialization failed")?
            .0
            .into_inner();
        let pod = Pod::from_bytes(&bytes).ok_or("capture PortConfig pod is invalid")?;
        let generation = {
            let _guard = self.connection.lock();
            let mut state = self
                .observation
                .lock()
                .map_err(|_| "capture preparation poisoned")?;
            state.reply.clear();
            let generation = state.generation;
            let node = self.node()?;
            // SAFETY: this preparation owns node/core and holds their loop lock;
            // the serialized pod remains readable through the native RPC call.
            let proxy = unsafe {
                pipewire::sys::pw_core_find_proxy(
                    self.connection.core().as_raw_ptr(),
                    node.upcast_ref().id(),
                )
            };
            if proxy.is_null() {
                return Err("capture node proxy was removed".into());
            }
            // SAFETY: the resolved proxy cannot be removed under this loop lock.
            // Acceptance is followed by PortConfig serial and exact-sequence
            // readback; a core roundtrip is not a manager completion barrier.
            let result = unsafe {
                pipewire::spa::spa_interface_call_method!(
                    proxy,
                    pipewire::sys::pw_node_methods,
                    set_param,
                    ParamType::PortConfig.as_raw(),
                    0,
                    pod.as_raw_ptr()
                )
            };
            SpaResult::from_c(result)
                .into_result()
                .map_err(|error| error.to_string())?;
            generation
        };
        Ok((wanted, generation))
    }
}

impl Observation {
    fn timeout_diagnostic(&self) -> String {
        use pipewire::spa::sys;
        let replies: Vec<String> = self
            .reply
            .iter()
            .map(|bytes| {
                let Ok((_, Value::Object(object))) = PodDeserializer::deserialize_any_from(bytes)
                else {
                    return format!("unparsed-pod({}bytes)", bytes.len());
                };
                format!(
                    "direction={:?} mode={:?} monitor={:?} format={:?}",
                    property(&object, sys::SPA_PARAM_PORT_CONFIG_direction),
                    property(&object, sys::SPA_PARAM_PORT_CONFIG_mode),
                    property(&object, sys::SPA_PARAM_PORT_CONFIG_monitor),
                    property(&object, sys::SPA_PARAM_PORT_CONFIG_format)
                )
            })
            .collect();
        format!(
            "capture PortConfig preparation timed out: node(id,state,input_ports,output_ports)={:?} serial={:?} generation={} expected_sequence={:?} last_sequence={:?} replies={} [{}]",
            self.node,
            self.serial,
            self.generation,
            self.sequence,
            self.last_sequence,
            self.reply.len(),
            replies.join("; ")
        )
    }
}

impl CaptureHost for Preparation<'_> {
    type Configuration = Object;

    fn snapshot(&self) -> Result<Snapshot<Object>, PreparationError> {
        let state = self
            .observation
            .lock()
            .map_err(|_| PreparationError::Host("capture preparation poisoned".into()))?;
        if let Some(error) = &state.error {
            return Err(PreparationError::Host(error.clone()));
        }
        Ok(Snapshot {
            generation: state.generation,
            configured: input_readback(&state.reply),
        })
    }
    fn request(&self) -> Result<(), PreparationError> {
        Preparation::request(self).map_err(PreparationError::Host)
    }
    fn monitoring(configuration: &Object) -> bool {
        property(
            configuration,
            pipewire::spa::sys::SPA_PARAM_PORT_CONFIG_monitor,
        ) == Some(&Value::Bool(true))
    }
    fn enable_monitor(
        &self,
        configuration: Object,
        rate: u32,
    ) -> Result<(Object, u64), PreparationError> {
        self.write_monitor(configuration, rate)
            .map_err(PreparationError::Host)
    }
    fn timeout(&self) -> PreparationError {
        let state = self
            .observation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        PreparationError::Host(state.timeout_diagnostic())
    }
}

pub(super) fn prepare(connection: &Connection, node_id: u32, rate: u32) -> Result<(), String> {
    let preparation = Preparation::new(connection, node_id)?;
    let clock = mixrelay_time::StdClock::new();
    negotiation::prepare(&preparation, &clock, rate).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrelated_sequence_and_unconfigured_ports_cannot_complete_startup() {
        let mut observation = Observation {
            sequence: Some(17),
            ..Observation::default()
        };
        observation.param(18, ParamType::PortConfig, None);
        assert!(observation.reply.is_empty());
        let config = Object {
            type_: pipewire::spa::sys::SPA_TYPE_OBJECT_ParamPortConfig,
            id: ParamType::PortConfig.as_raw(),
            properties: Vec::new(),
        };
        assert!(!configured_input(&config));
        let monitor = monitor_config(config);
        assert_eq!(
            property(&monitor, pipewire::spa::sys::SPA_PARAM_PORT_CONFIG_monitor),
            Some(&Value::Bool(true))
        );
        let diagnostic = observation.timeout_diagnostic();
        assert!(diagnostic.contains("expected_sequence=Some(17)"));
        assert!(diagnostic.contains("last_sequence=Some(18)"));
        assert!(diagnostic.contains("replies=0"));
    }

    #[test]
    fn manager_singleton_direction_and_clock_selected_format_are_ready() {
        use pipewire::spa::{
            sys,
            utils::{Choice, ChoiceFlags},
        };
        let direction = Value::Choice(ChoiceValue::Id(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::None(Id(sys::SPA_DIRECTION_INPUT)),
        )));
        let config = Object {
            type_: sys::SPA_TYPE_OBJECT_ParamPortConfig,
            id: ParamType::PortConfig.as_raw(),
            properties: vec![
                Property::new(sys::SPA_PARAM_PORT_CONFIG_direction, direction),
                Property::new(
                    sys::SPA_PARAM_PORT_CONFIG_mode,
                    Value::Id(Id(sys::SPA_PARAM_PORT_CONFIG_MODE_dsp)),
                ),
                Property::new(sys::SPA_PARAM_PORT_CONFIG_monitor, Value::Bool(true)),
                Property::new(
                    sys::SPA_PARAM_PORT_CONFIG_format,
                    Value::Object(Object {
                        type_: sys::SPA_TYPE_OBJECT_Format,
                        id: ParamType::Format.as_raw(),
                        properties: vec![Property::new(
                            sys::SPA_FORMAT_AUDIO_channels,
                            Value::Int(2),
                        )],
                    }),
                ),
            ],
        };
        assert!(configured_input(&config));
        let writable = writable_config(config, 48_000).unwrap();
        let Some(Value::Object(format)) = property(&writable, sys::SPA_PARAM_PORT_CONFIG_format)
        else {
            panic!("format lost")
        };
        assert_eq!(
            property(format, sys::SPA_FORMAT_AUDIO_rate),
            Some(&Value::Int(48_000))
        );
    }

    #[test]
    fn port_config_readback_preserves_format_and_requires_its_exact_sequence() {
        use pipewire::spa::sys;

        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Observation>();
        let format = Value::Object(Object {
            type_: sys::SPA_TYPE_OBJECT_Format,
            id: ParamType::Format.as_raw(),
            properties: vec![Property::new(sys::SPA_FORMAT_AUDIO_rate, Value::Int(0))],
        });
        let config = Object {
            type_: sys::SPA_TYPE_OBJECT_ParamPortConfig,
            id: ParamType::PortConfig.as_raw(),
            properties: vec![
                Property::new(
                    sys::SPA_PARAM_PORT_CONFIG_direction,
                    Value::Id(Id(sys::SPA_DIRECTION_INPUT)),
                ),
                Property::new(
                    sys::SPA_PARAM_PORT_CONFIG_mode,
                    Value::Id(Id(sys::SPA_PARAM_PORT_CONFIG_MODE_dsp)),
                ),
                Property::new(sys::SPA_PARAM_PORT_CONFIG_format, format.clone()),
            ],
        };
        let wanted = monitor_config(config);
        assert_eq!(
            property(&wanted, sys::SPA_PARAM_PORT_CONFIG_format),
            Some(&format)
        );
        let writable = writable_config(wanted.clone(), 48_000).unwrap();
        let Some(Value::Object(written_format)) =
            property(&writable, sys::SPA_PARAM_PORT_CONFIG_format)
        else {
            panic!("format lost")
        };
        assert_eq!(
            property(written_format, sys::SPA_FORMAT_AUDIO_rate),
            Some(&Value::Int(48_000))
        );
        // The ACK expectation retains DSP's clock-selected rate, not the
        // positive placeholder required only by the PortConfig setter.
        assert_eq!(
            property(&wanted, sys::SPA_PARAM_PORT_CONFIG_format),
            Some(&format)
        );
        let bytes =
            PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(wanted.clone()))
                .unwrap()
                .0
                .into_inner();
        let pod = Pod::from_bytes(&bytes).unwrap();
        let mut observation = Observation {
            sequence: Some(19),
            ..Observation::default()
        };
        observation.param(18, ParamType::PortConfig, Some(pod));
        assert!(observation.reply.is_empty());
        observation.param(19, ParamType::PortConfig, Some(pod));
        assert_eq!(input_readback(&observation.reply), Some(wanted));
        observation.serial(5);
        let prepared = observation.generation;
        observation.serial(5); // Duplicate info/Start cannot manufacture a preparation completion.
        assert_eq!(observation.generation, prepared);
        observation.param(19, ParamType::Format, Some(pod));
        assert_eq!(observation.generation, prepared);
    }
}
