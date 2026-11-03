use super::*;
use crate::{
    AudioLayout, HostedProcessorControl, RackLocator, WORKER_READY,
    catalog::{CatalogEntry, RackCatalogTarget},
    control::ProcessorControlRegistration,
    preparation::{PreparedProcessor, PreparedWorker, WorkerPreparationError},
    shared_transport::memory,
    state_blobs::memory::MemoryBlobs,
};
use mixrelay_audio_graph::{NodeId, ProviderNodeConfig, ProviderPorts, RenderFormat};
use mixrelay_control_protocol::{ExternalProcessorDescriptor, ExternalProcessorFormat};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Preparation {
    calls: Mutex<Vec<ProviderNodeConfig>>,
    fail: AtomicBool,
    identities: Mutex<Vec<crate::instance_id::InstanceId>>,
    force_identity: AtomicU64,
    released: Arc<AtomicUsize>,
}

impl WorkerPreparation for Preparation {
    fn prepare(
        &self,
        _: &CatalogEntry,
        config: &ProviderNodeConfig,
        _: RenderFormat,
        instance_id: crate::instance_id::InstanceId,
    ) -> Result<PreparedWorker, WorkerPreparationError> {
        self.calls.lock().unwrap().push(config.clone());
        self.identities.lock().unwrap().push(instance_id);
        if self.fail.load(Ordering::Relaxed) {
            return Err(WorkerPreparationError::Native {
                diagnostic: "injected worker preparation failure".into(),
            });
        }
        let instance_id =
            crate::instance_id::InstanceId::new(self.force_identity.load(Ordering::Relaxed))
                .unwrap_or(instance_id);
        let (_, mailbox, _, _) = memory::endpoints(
            AudioLayout {
                inputs: 2,
                outputs: 2,
                block_frames: 32,
            },
            instance_id,
        );
        let control = Arc::new(HostedProcessorControl::native(
            mailbox,
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        ));
        control
            .shared
            .header()
            .worker_status
            .store(WORKER_READY, Ordering::Release);
        Ok(PreparedWorker {
            processor: Box::new(Owner {
                control: Arc::clone(&control),
                registration: None,
                released: Arc::clone(&self.released),
            }),
            control,
        })
    }
}

struct Owner {
    control: Arc<HostedProcessorControl>,
    registration: Option<ProcessorControlRegistration>,
    released: Arc<AtomicUsize>,
}

impl PreparedProcessor for Owner {
    fn register(
        mut self: Box<Self>,
        registration: ProcessorControlRegistration,
    ) -> Box<dyn HostedAudioProcessor> {
        self.registration = Some(registration);
        self
    }
}

impl HostedAudioProcessor for Owner {
    fn process(&mut self, _: &[f32], _: &mut [f32], _: usize) -> bool {
        false
    }
    fn set_active(&mut self, active: bool) {
        self.control.active_owner.store(active, Ordering::Release);
    }
    fn reuse_identity(&self) -> Option<&mixrelay_audio_engine::HostedProcessorIdentity> {
        Some(&self.control.adoption_identity)
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        drop(self.registration.take());
        self.released.fetch_add(1, Ordering::Relaxed);
    }
}

fn catalog() -> ProcessorCatalog {
    let catalog = ProcessorCatalog::default();
    let ports = ProviderPorts::canonical_stereo("selected-stereo");
    let descriptor = ExternalProcessorDescriptor {
        type_id: "clap:fixture".into(),
        display_name: "Fixture".into(),
        vendor: String::new(),
        version: String::new(),
        format: ExternalProcessorFormat::Clap,
        layouts: vec![ports.clone()],
        default_layout_id: ports.layout_id,
        has_editor: false,
        parameters: Vec::new(),
    };
    let locator: RackLocator = serde_json::from_value(serde_json::json!({
        "format":"clap", "uniqueId":"fixture", "path":"/fixture.clap", "name":"Fixture",
        "vendor":"fixture", "version":1, "category":"effect", "hasEditor":false, "acceptsMidi":false
    }))
    .unwrap();
    catalog.snapshot.write().unwrap().entries.insert(
        descriptor.type_id.clone(),
        CatalogEntry {
            descriptor,
            rack: RackCatalogTarget { locator },
        },
    );
    catalog
}

fn config() -> ProviderNodeConfig {
    ProviderNodeConfig {
        type_id: "clap:fixture".into(),
        ports: None,
        parameters: serde_json::json!({}),
    }
}

fn request<'a>(
    node: &'a NodeId,
    config: &'a ProviderNodeConfig,
) -> HostedProcessorPrepareRequest<'a> {
    HostedProcessorPrepareRequest {
        node_id: node,
        config,
        format: RenderFormat {
            sample_rate: 48_000,
            block_frames: 32,
        },
        input_channels: 2,
        output_channels: 2,
    }
}

fn fixture(preparation: &Arc<Preparation>) -> (ExternalProcessorFactory, ProcessorControlRegistry) {
    let controls = ProcessorControlRegistry::new(Arc::new(MemoryBlobs::default()));
    let factory = ExternalProcessorFactory::with_preparation(
        catalog(),
        controls.clone(),
        preparation.clone(),
    );
    (factory, controls)
}

#[test]
fn rejected_state_or_compiled_layout_never_prepares_a_worker() {
    let preparation = Arc::new(Preparation::default());
    let (factory, controls) = fixture(&preparation);
    let node = NodeId::from("effect");
    let mut config = config();
    config.parameters = serde_json::json!({"stateRef":{"sha256":"0".repeat(64),"byteLength":3}});
    assert!(factory.prepare(request(&node, &config)).is_err());
    config.parameters = serde_json::json!({});
    let mut wrong_layout = request(&node, &config);
    wrong_layout.input_channels = 4;
    assert!(factory.prepare(wrong_layout).is_err());
    assert!(preparation.calls.lock().unwrap().is_empty());
    assert!(!controls.has_worker(&node));
}

#[test]
fn failed_preparation_does_not_register_and_the_next_attempt_can_succeed() {
    let preparation = Arc::new(Preparation::default());
    let (factory, controls) = fixture(&preparation);
    let node = NodeId::from("effect");
    let config = config();
    preparation.fail.store(true, Ordering::Relaxed);
    assert_eq!(
        factory.prepare(request(&node, &config)).err().unwrap(),
        "injected worker preparation failure"
    );
    assert!(!controls.has_worker(&node));
    preparation.fail.store(false, Ordering::Relaxed);
    let processor = factory.prepare(request(&node, &config)).unwrap();
    assert!(controls.has_worker(&node));
    assert_eq!(preparation.calls.lock().unwrap().len(), 2);
    assert_eq!(
        preparation
            .identities
            .lock()
            .unwrap()
            .iter()
            .map(|id| id.get())
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(
        preparation.calls.lock().unwrap()[1].ports,
        factory.catalog.ports("clap:fixture")
    );
    drop(processor);
    assert!(!controls.has_worker(&node));
    assert_eq!(preparation.released.load(Ordering::Relaxed), 1);
}

#[test]
fn exact_active_reuse_skips_preparation_but_changed_state_starts_a_new_owner() {
    let preparation = Arc::new(Preparation::default());
    let (factory, controls) = fixture(&preparation);
    let node = NodeId::from("effect");
    let mut config = config();
    let mut original = factory.prepare(request(&node, &config)).unwrap();
    original.set_active(true);
    let identity = original.reuse_identity();
    let reused = factory
        .prepare_inactive(request(&node, &config))
        .unwrap()
        .unwrap();
    assert_eq!(reused.reuse_identity(), identity);
    assert_eq!(preparation.calls.lock().unwrap().len(), 1);
    config.parameters = serde_json::json!({"parameterValues":{"7":0.4}});
    let changed = factory.prepare(request(&node, &config)).unwrap();
    assert_ne!(changed.reuse_identity(), identity);
    assert_eq!(preparation.calls.lock().unwrap().len(), 2);
    assert_eq!(
        preparation
            .identities
            .lock()
            .unwrap()
            .iter()
            .map(|id| id.get())
            .collect::<Vec<_>>(),
        [1, 2]
    );
    drop(reused);
    drop(original);
    assert!(controls.has_worker(&node));
    drop(changed);
    assert!(!controls.has_worker(&node));
    assert_eq!(preparation.released.load(Ordering::Relaxed), 2);
}

#[test]
fn factories_and_clones_share_the_registry_identity_owner() {
    let preparation = Arc::new(Preparation::default());
    let (first, controls) = fixture(&preparation);
    let second =
        ExternalProcessorFactory::with_preparation(catalog(), controls, preparation.clone());
    let clone = first.clone();
    let config = config();
    let mut owners = Vec::new();
    for (factory, node) in [(&first, "a"), (&second, "b"), (&clone, "c")] {
        owners.push(
            factory
                .prepare(request(&NodeId::from(node), &config))
                .unwrap(),
        );
    }
    assert_eq!(
        preparation
            .identities
            .lock()
            .unwrap()
            .iter()
            .map(|id| id.get())
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

#[test]
fn mismatched_prepared_identity_drops_the_owner_before_registration() {
    let preparation = Arc::new(Preparation::default());
    let (factory, controls) = fixture(&preparation);
    let node = NodeId::from("effect");
    let config = config();
    preparation.force_identity.store(999, Ordering::Relaxed);
    let error = factory.prepare(request(&node, &config)).err().unwrap();
    assert_eq!(
        error,
        "prepared processor instance 999 does not match issued instance 1"
    );
    assert!(!controls.has_worker(&node));
    assert_eq!(preparation.released.load(Ordering::Relaxed), 1);
    preparation.force_identity.store(0, Ordering::Relaxed);
    let owner = factory.prepare(request(&node, &config)).unwrap();
    assert_eq!(
        controls
            .live_control(&node)
            .unwrap()
            .shared
            .header()
            .instance_id,
        2
    );
    assert!(controls.has_worker(&node));
    drop(owner);
    assert!(!controls.has_worker(&node));
    assert_eq!(preparation.released.load(Ordering::Relaxed), 2);
}

#[test]
fn exhausted_registry_rejects_new_preparation_but_keeps_exact_active_reuse() {
    let preparation = Arc::new(Preparation::default());
    let mut controls = ProcessorControlRegistry::new(Arc::new(MemoryBlobs::default()));
    controls.instances = Arc::new(crate::instance_id::InstanceIds::final_available());
    let factory = ExternalProcessorFactory::with_preparation(
        catalog(),
        controls.clone(),
        preparation.clone(),
    );
    let node = NodeId::from("effect");
    let original_config = config();
    let mut owner = factory.prepare(request(&node, &original_config)).unwrap();
    owner.set_active(true);
    assert_eq!(
        controls
            .live_control(&node)
            .unwrap()
            .shared
            .header()
            .instance_id,
        u64::MAX
    );
    let mut changed = original_config.clone();
    changed.parameters = serde_json::json!({"parameterValues":{"7":0.4}});
    assert_eq!(
        factory.prepare(request(&node, &changed)).err().unwrap(),
        "processor instance identifiers are exhausted"
    );
    assert!(controls.has_worker(&node));
    let reused = factory
        .prepare_inactive(request(&node, &original_config))
        .unwrap()
        .unwrap();
    assert_eq!(reused.reuse_identity(), owner.reuse_identity());
    assert_eq!(preparation.calls.lock().unwrap().len(), 1);
}

struct PreparedOwner(Mutex<Option<Box<dyn HostedAudioProcessor>>>);

impl HostedProcessorFactory for PreparedOwner {
    fn prepare(
        &self,
        _: HostedProcessorPrepareRequest<'_>,
    ) -> Result<Box<dyn HostedAudioProcessor>, String> {
        Ok(self.0.lock().unwrap().take().unwrap())
    }
}

fn render_holder(
    revision: u64,
    processor: Box<dyn HostedAudioProcessor>,
) -> Box<mixrelay_audio_engine::BlockProcessor> {
    let node = mixrelay_audio_graph::NodeSpec {
        id: NodeId::from("effect"),
        label: None,
        control: mixrelay_audio_graph::NodeControl::default(),
        config: mixrelay_audio_graph::NodeConfig::Provider(config()),
    };
    let compiler =
        mixrelay_audio_graph::GraphCompiler::new(crate::CatalogNodeResolver::new(catalog()));
    let execution = compiler
        .compile(
            &mixrelay_audio_graph::GraphDocument::new(revision, vec![node], vec![]),
            mixrelay_audio_graph::CompileContext {
                format: RenderFormat {
                    sample_rate: 48_000,
                    block_frames: 32,
                },
                limits: mixrelay_audio_graph::CompileLimits::default(),
            },
        )
        .unwrap();
    let owner = PreparedOwner(Mutex::new(Some(processor)));
    let prepared = mixrelay_audio_engine::BlockProcessor::prepare_with_hosted_processors(
        &execution,
        Arc::new(mixrelay_audio_engine::PeakMeterBank::new(8, 2)),
        &owner,
    )
    .unwrap();
    assert!(owner.0.lock().unwrap().is_none());
    Box::new(prepared)
}

#[test]
fn independent_registries_never_adopt_each_others_numeric_instance() {
    let first_preparation = Arc::new(Preparation::default());
    let second_preparation = Arc::new(Preparation::default());
    let (first_factory, first_controls) = fixture(&first_preparation);
    let (second_factory, second_controls) = fixture(&second_preparation);
    let node = NodeId::from("effect");
    let config = config();
    let mut original = first_factory.prepare(request(&node, &config)).unwrap();
    let mut foreign = second_factory.prepare(request(&node, &config)).unwrap();
    original.set_active(true);
    foreign.set_active(true);
    assert_eq!(
        first_controls
            .live_control(&node)
            .unwrap()
            .shared
            .header()
            .instance_id,
        1
    );
    assert_eq!(
        second_controls
            .live_control(&node)
            .unwrap()
            .shared
            .header()
            .instance_id,
        1
    );
    let foreign_reuse = second_factory
        .prepare_inactive(request(&node, &config))
        .unwrap()
        .unwrap();
    let mut current = render_holder(1, original);
    let exchange = mixrelay_audio_engine::ProcessorExchange::new(1);
    exchange.submit(render_holder(2, foreign_reuse)).unwrap();
    assert!(
        !exchange
            .activate_pending_configured(&mut current, |_| -> Result<(), ()> {
                panic!("a foreign instance must not reach native activation");
            })
            .unwrap()
    );
    assert_eq!(exchange.revisions().active, 1);
    assert_eq!(first_preparation.released.load(Ordering::Relaxed), 0);
    assert_eq!(second_preparation.released.load(Ordering::Relaxed), 0);
}
