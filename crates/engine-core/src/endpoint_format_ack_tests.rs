use super::*;
use crate::controller_test_support::settle;
use mixrelay_control_protocol::{
    GraphCatalogAction, GraphCatalogRequest, GraphLayout, GraphLayoutRequest, GraphNodePosition,
};

use std::collections::VecDeque;

struct Plan {
    observations: VecDeque<Option<u32>>,
    completes: bool,
}

impl Plan {
    fn completes(observations: &[Option<u32>]) -> Self {
        Self {
            observations: observations.iter().copied().collect(),
            completes: true,
        }
    }
}

struct Pending {
    previous: u32,
    target: u32,
    plan: Plan,
}

struct Negotiation {
    pending: Option<Pending>,
    forward: Option<Plan>,
    rollback: Option<Plan>,
    cancelled: u32,
}

struct AsynchronousEndpoint {
    base: Arc<dyn VirtualEndpointBackend>,
    state: Arc<NativeState>,
    negotiation: Mutex<Negotiation>,
}

impl VirtualEndpointBackend for AsynchronousEndpoint {
    fn capabilities(&self) -> &VirtualEndpointCapabilities {
        self.base.capabilities()
    }

    fn settings(&self) -> Option<VirtualEndpointSettings> {
        self.base.settings()
    }

    fn prepare_graph_attachments(
        &self,
        specs: &[VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError> {
        self.base.prepare_graph_attachments(specs)
    }

    fn sample_rate(&self, id: &VirtualEndpointId) -> Result<u32, HostAudioBackendError> {
        if id.as_str() != DEFAULT_ENDPOINT_NODE_ID {
            return self.base.sample_rate(id);
        }
        let mut negotiation = self.negotiation.lock();
        let Some(mut pending) = negotiation.pending.take() else {
            return Ok(self.state.rate.load(Ordering::Acquire));
        };
        let observed = pending.plan.observations.pop_front().unwrap_or({
            Some(if pending.plan.completes {
                pending.target
            } else {
                pending.previous
            })
        });
        if let Some(rate) = observed {
            self.state.rate.store(rate, Ordering::Release);
        }
        // A third value models a DAW superseding this generation. A target value
        // acknowledges it; only the old/missing observation leaves it pending.
        if observed != Some(pending.target)
            && (observed.is_none() || observed == Some(pending.previous))
        {
            negotiation.pending = Some(pending);
        }
        observed.ok_or_else(|| HostAudioBackendError::BackendFailure {
            message: "device configuration temporarily unavailable".into(),
        })
    }

    fn set_sample_rate(
        &self,
        id: &VirtualEndpointId,
        rate: u32,
    ) -> Result<(), HostAudioBackendError> {
        assert_eq!(id.as_str(), DEFAULT_ENDPOINT_NODE_ID);
        self.state.calls.fetch_add(1, Ordering::Relaxed);
        self.state.writes.lock().push(id.clone());
        let mut negotiation = self.negotiation.lock();
        let cancelled = u32::from(negotiation.pending.take().is_some());
        negotiation.cancelled += cancelled;
        let plan = if rate == request().sample_rate {
            negotiation.forward.take()
        } else {
            negotiation.rollback.take()
        }
        .unwrap_or_else(|| Plan::completes(&[]));
        negotiation.pending = Some(Pending {
            previous: self.state.rate.load(Ordering::Acquire),
            target: rate,
            plan,
        });
        Ok(())
    }
}

fn asynchronous(
    forward: Plan,
    rollback: Plan,
) -> (EngineGraphController, Arc<AsynchronousEndpoint>) {
    let (mut controller, state) = fixture();
    let backend = Arc::new(AsynchronousEndpoint {
        base: controller.endpoint_backend.clone(),
        state,
        negotiation: Mutex::new(Negotiation {
            pending: None,
            forward: Some(forward),
            rollback: Some(rollback),
            cancelled: 0,
        }),
    });
    controller.endpoint_backend = backend.clone();
    (controller, backend)
}

#[test]
fn accepted_request_waits_through_old_rate_and_temporary_absence_before_publication() {
    let (controller, backend) = asynchronous(
        Plan::completes(&[Some(48_000), None, Some(48_000)]),
        Plan::completes(&[]),
    );
    let result = controller.change_endpoint_format(&request()).unwrap();
    assert_eq!(result.revision, 2);
    assert_eq!(backend.state.rate.load(Ordering::Acquire), 44_100);
    assert_eq!(backend.state.calls.load(Ordering::Acquire), 1);
    assert_eq!(backend.state.resumed.load(Ordering::Acquire), 1);
    assert!(!backend.state.failed.load(Ordering::Acquire));
    assert!(backend.negotiation.lock().pending.is_none());
    assert_eq!(controller.context.read().format.sample_rate, 48_000);
}

#[test]
fn timeout_supersedes_the_uncommitted_generation_even_when_readback_is_still_previous() {
    let (controller, backend) = asynchronous(
        Plan {
            observations: VecDeque::new(),
            completes: false,
        },
        Plan::completes(&[]),
    );
    let error = controller.change_endpoint_format(&request()).unwrap_err();
    assert!(error.parameters.to_string().contains("timed out"));
    assert_eq!(backend.state.calls.load(Ordering::Acquire), 2);
    assert_eq!(backend.negotiation.lock().cancelled, 1);
    assert!(backend.negotiation.lock().pending.is_none());
    assert_eq!(backend.state.rate.load(Ordering::Acquire), 48_000);
    assert_eq!(backend.state.resumed.load(Ordering::Acquire), 1);
    assert!(!backend.state.failed.load(Ordering::Acquire));
    assert_eq!(controller.graph().revision, 1);
}

#[test]
fn third_rate_during_acknowledgement_is_not_overwritten_by_rollback() {
    let (controller, backend) =
        asynchronous(Plan::completes(&[None, Some(96_000)]), Plan::completes(&[]));
    assert!(controller.change_endpoint_format(&request()).is_err());
    assert_eq!(backend.state.rate.load(Ordering::Acquire), 96_000);
    assert_eq!(backend.state.calls.load(Ordering::Acquire), 1);
    assert_eq!(backend.state.resumed.load(Ordering::Acquire), 0);
    assert!(backend.state.failed.load(Ordering::Acquire));
    assert_eq!(controller.graph().revision, 1);
}

#[test]
fn rejected_graph_waits_for_asynchronous_rollback_before_resuming() {
    let (mut controller, backend) = asynchronous(
        Plan::completes(&[None]),
        Plan::completes(&[Some(44_100), None]),
    );
    controller.graph_resources = Arc::new(Reject);
    assert!(controller.change_endpoint_format(&request()).is_err());
    assert_eq!(backend.state.rate.load(Ordering::Acquire), 48_000);
    assert_eq!(backend.state.calls.load(Ordering::Acquire), 2);
    assert_eq!(backend.state.resumed.load(Ordering::Acquire), 1);
    assert!(!backend.state.failed.load(Ordering::Acquire));
    assert_eq!(controller.graph().revision, 1);
}

struct MasterClockRuntime {
    base: Arc<dyn RuntimeBackend>,
    endpoint: Arc<AsynchronousEndpoint>,
    reject_adoption: bool,
    adoptions: AtomicU32,
    resumes: Mutex<Vec<(u32, bool)>>,
}

impl RuntimeStatusBackend for MasterClockRuntime {
    fn status(&self, format: RenderFormat) -> AppStatus {
        self.base.status(format)
    }
}

impl ProcessingControlBackend for MasterClockRuntime {
    fn suspend(&self) -> Result<(), HostAudioBackendError> {
        self.base.suspend()
    }
    fn fail_reconfiguration(&self) {
        self.base.fail_reconfiguration();
    }
    fn resume(&self) {
        self.resumes.lock().push((
            self.endpoint.state.rate.load(Ordering::Acquire),
            self.endpoint.negotiation.lock().pending.is_some(),
        ));
        self.base.resume();
    }
}

impl SessionFormatBackend for MasterClockRuntime {
    fn external_format(&self, current: RenderFormat) -> Option<RenderFormat> {
        Some(RenderFormat {
            sample_rate: self.endpoint.state.rate.load(Ordering::Acquire),
            ..current
        })
    }
    fn adopt_external_format(&self, requested: RenderFormat) -> Result<(), HostAudioBackendError> {
        self.adoptions.fetch_add(1, Ordering::Relaxed);
        assert_eq!(requested.sample_rate, 44_100);
        assert_eq!(self.endpoint.state.resumed.load(Ordering::Acquire), 0);
        if self.reject_adoption {
            return Err(HostAudioBackendError::BackendFailure {
                message: "master clock adoption failed after outer suspension".into(),
            });
        }
        Ok(())
    }
}

fn master_runtime(
    controller: &mut EngineGraphController,
    endpoint: Arc<AsynchronousEndpoint>,
    reject_adoption: bool,
) -> Arc<MasterClockRuntime> {
    let runtime = Arc::new(MasterClockRuntime {
        base: controller.runtime_status.clone(),
        endpoint,
        reject_adoption,
        adoptions: AtomicU32::new(0),
        resumes: Mutex::new(Vec::new()),
    });
    controller.runtime_status = runtime.clone();
    runtime
}

#[test]
fn master_clock_adoption_failure_cannot_resume_before_outer_rollback_acknowledgement() {
    let (mut controller, backend) = asynchronous(
        Plan::completes(&[Some(48_000), None]),
        Plan::completes(&[Some(44_100), None, Some(44_100)]),
    );
    let runtime = master_runtime(&mut controller, backend.clone(), true);
    assert!(controller.change_endpoint_format(&request()).is_err());
    assert_eq!(runtime.adoptions.load(Ordering::Acquire), 1);
    assert_eq!(*runtime.resumes.lock(), vec![(48_000, false)]);
    assert_eq!(backend.state.calls.load(Ordering::Acquire), 2);
    assert_eq!(backend.state.resumed.load(Ordering::Acquire), 1);
    assert!(!backend.state.failed.load(Ordering::Acquire));
    assert_eq!(controller.context.read().format.sample_rate, 48_000);
    assert_eq!(controller.graph().revision, 1);
}

#[test]
fn successful_master_clock_change_has_one_outer_resume_after_publication() {
    let (mut controller, backend) =
        asynchronous(Plan::completes(&[Some(48_000), None]), Plan::completes(&[]));
    let runtime = master_runtime(&mut controller, backend.clone(), false);
    let response = controller.change_endpoint_format(&request()).unwrap();
    assert_eq!(runtime.adoptions.load(Ordering::Acquire), 1);
    assert_eq!(*runtime.resumes.lock(), vec![(44_100, false)]);
    assert_eq!(backend.state.calls.load(Ordering::Acquire), 1);
    assert_eq!(backend.state.resumed.load(Ordering::Acquire), 1);
    assert_eq!(controller.context.read().format.sample_rate, 44_100);
    assert_eq!(controller.graph().revision, response.revision);
}
fn view(controller: &EngineGraphController, action: GraphCatalogAction) {
    let before = controller.exchange.revisions();
    controller
        .mutate_graph_catalog(GraphCatalogRequest {
            expected_revision: controller.graph().revision,
            expected_catalog_revision: controller.graph_catalog().unwrap().catalog_revision,
            action,
        })
        .unwrap();
    assert_eq!(controller.exchange.revisions(), before);
}

fn add_shared_reference(controller: &EngineGraphController) {
    let original = controller.graph();
    let first_id = controller.graph_catalog().unwrap().active_id;
    view(
        controller,
        GraphCatalogAction::Create {
            name: "Second graph".into(),
        },
    );
    let mut second = original;
    second.revision = controller.exchange.revisions().submitted + 1;
    second.edges.clear();
    let NodeConfig::VirtualEndpoint(config) = &mut second.nodes[0].config else {
        panic!("virtual reference")
    };
    config.reference.capture = false;
    config.gain_db = -9.0;
    controller.apply_graph_request(second.into()).unwrap();
    settle(controller);
    controller
        .update_graph_layout(
            &first_id,
            GraphLayoutRequest {
                expected_layout_revision: 0,
                layout: GraphLayout {
                    node_labels: std::collections::BTreeMap::default(),
                    positions: [(
                        DEFAULT_ENDPOINT_NODE_ID.into(),
                        GraphNodePosition { x: 12.0, y: 34.0 },
                    )]
                    .into(),
                    viewport: None,
                    sizes: std::collections::BTreeMap::default(),
                },
            },
        )
        .unwrap();
    view(controller, GraphCatalogAction::Select { id: first_id });
}

#[test]
fn asynchronous_device_acknowledgement_updates_every_catalog_reference_in_one_publication() {
    let directory = crate::test_support::Documents::new("async-device-rate-catalog");
    let path = directory.join("graphs.json");
    let (controller, backend) = asynchronous(
        Plan::completes(&[Some(48_000), None, Some(48_000)]),
        Plan::completes(&[]),
    );
    let controller = controller
        .with_test_documents(&directory.files)
        .with_graph_catalog_path(path.clone());
    add_shared_reference(&controller);
    let before: serde_json::Value = serde_json::from_slice(
        &crate::test_support::read_document(&directory.files, &path).unwrap(),
    )
    .unwrap();
    let mut change = request();
    change.expected_revision = controller.graph().revision;
    controller.change_endpoint_format(&change).unwrap();
    let after: serde_json::Value = serde_json::from_slice(
        &crate::test_support::read_document(&directory.files, &path).unwrap(),
    )
    .unwrap();
    let before_graphs = before["graphs"].as_array().unwrap();
    let after_graphs = after["graphs"].as_array().unwrap();
    assert_eq!(after_graphs.len(), 2);
    for (before, after) in before_graphs.iter().zip(after_graphs) {
        assert_eq!(before["layout"], after["layout"]);
        assert_eq!(before["name"], after["name"]);
        let mut expected: GraphDocument =
            serde_json::from_value(before["document"].clone()).unwrap();
        let actual: GraphDocument = serde_json::from_value(after["document"].clone()).unwrap();
        expected.revision = actual.revision;
        let NodeConfig::VirtualEndpoint(config) = &mut expected.nodes[0].config else {
            panic!("shared device")
        };
        config.default_sample_rate = 44_100;
        config.supported_sample_rates = vec![44_100, 48_000];
        assert_eq!(actual, expected);
        assert_eq!(
            controller
                .graph_catalog_document(after["id"].as_str().unwrap())
                .unwrap(),
            actual
        );
    }
    assert_eq!(backend.state.calls.load(Ordering::Acquire), 1);
    assert_eq!(controller.context.read().format.sample_rate, 48_000);
}
