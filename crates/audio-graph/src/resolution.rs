//! Graph compilation inputs and the consumer-owned node resolution port.

use crate::model::{CompiledPort, NodeSpec, RenderFormat};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompileContext {
    pub format: RenderFormat,
    pub limits: CompileLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompileLimits {
    pub max_nodes: usize,
    pub max_edges: usize,
    pub max_channels_per_port: u16,
    pub max_sample_rate: u32,
    pub max_block_frames: u32,
    pub max_plan_bytes: usize,
}

impl Default for CompileLimits {
    fn default() -> Self {
        Self {
            max_nodes: 256,
            max_edges: 1_024,
            max_channels_per_port: 64,
            max_sample_rate: 384_000,
            max_block_frames: 4_096,
            max_plan_bytes: 8 * 1_024 * 1_024,
        }
    }
}

pub trait NodeResolver: Send + Sync {
    /// Resolves a persisted node into ports available on the current system.
    ///
    /// # Errors
    ///
    /// Returns [`NodeResolutionError`] when configuration is invalid or a
    /// runtime capability required by the node is unavailable.
    fn resolve(
        &self,
        node: &NodeSpec,
        context: &CompileContext,
    ) -> Result<ResolvedNode, NodeResolutionError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedNode {
    pub inputs: Vec<CompiledPort>,
    pub outputs: Vec<CompiledPort>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NodeResolutionError {
    #[error("invalid node configuration: {reason}")]
    InvalidConfiguration { reason: String },
    #[error("node capability is unavailable: {reason}")]
    CapabilityUnavailable { reason: String },
}
