//! The desktop's side of the engine's HTTP control protocol: routes, client operation
//! receipts, failure reports, and what a response means for capture ownership. The caller
//! owns the transport.
pub mod event_stream;
pub mod failure;
pub mod receipt;
pub mod rejection;
pub mod routes;
pub mod settlement;

/// Whether a request reached the engine, as far as the desktop can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Delivery {
    /// The engine answered; its status and body describe the outcome.
    #[default]
    ServerResponse,
    /// The desktop refused or could not connect; the engine never saw the request.
    NotSent,
    /// The request may have reached the engine, but no answer was read.
    Uncertain,
}
