//! Current shared source registry contract; the Rust registry owns every map.
pub(super) use mixrelay_macos_transport::source_registry::{
    SourceRegistry, mr_engine_source_registry_clock, mr_engine_source_registry_read_clocked,
    mr_engine_source_registry_set_clock_master, mr_engine_source_registry_take_source_faults,
};
