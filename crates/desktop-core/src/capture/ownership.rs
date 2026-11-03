//! Control-thread capture ownership. Preparing never edits the active set;
//! rejecting a remote graph transaction drops only the provisional owners.
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use mixrelay_audio_graph::{ApplicationCaptureRequest, ApplicationSelector, NodeId};
use mixrelay_control_protocol::{ApplicationCaptureManifest, PROTOCOL_VERSION};

/// Invalid manifests, unresolved revisions, and preparation capacity failures.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("unsupported application capture manifest")]
    UnsupportedManifest,
    #[error("invalid canonical application capture identity or policy")]
    InvalidIdentity,
    #[error(transparent)]
    Contract(#[from] mixrelay_audio_io::HostAudioContractError),
    #[error("too many unacknowledged application capture revisions")]
    UnacknowledgedLimit,
    #[error("application capture mutation outcome is still unresolved")]
    UnresolvedMutation,
    #[error("capture retirement reservation failed: {0}")]
    Reservation(#[source] std::collections::TryReserveError),
    #[error("application capture reservation is missing")]
    MissingReservation,
    #[error("stale application capture manifest")]
    StaleManifest,
    #[error("one capture revision has conflicting source definitions")]
    ConflictingRevision,
}

/// A failed native start and all failures while restoring earlier starts.
#[derive(Debug)]
pub struct ActivationError<E> {
    pub start: E,
    pub rollback: Vec<E>,
}
impl<E: std::fmt::Display> std::fmt::Display for ActivationError<E> {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(output, "{}", self.start)?;
        for error in &self.rollback {
            write!(output, "; rollback failed: {error}")?;
        }
        Ok(())
    }
}
impl<E: std::error::Error + 'static> std::error::Error for ActivationError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.start)
    }
}

/// A manifest failure or the original native preparation error.
#[derive(Debug, thiserror::Error)]
pub enum PrepareError<E> {
    #[error(transparent)]
    Manifest(#[from] CaptureError),
    #[error("{0}")]
    Open(#[source] E),
}

#[derive(Debug)]
struct Entry<S> {
    node_id: String,
    request: ApplicationCaptureRequest,
    session: Arc<S>,
    activate: bool,
}

#[derive(Debug)]
pub struct CaptureSet<S> {
    revision: u64,
    sources: Vec<Entry<S>>,
}

impl<S> CaptureSet<S> {
    /// Prepared sessions in canonical manifest order.
    fn sessions(&self) -> impl Iterator<Item = &Arc<S>> {
        self.sources.iter().map(|entry| &entry.session)
    }
}

/// Prepares provisional sessions while retaining every active source.
/// # Errors
/// Returns a rejected manifest or the opener's original failure.
pub fn prepare<S, E>(
    manifest: &ApplicationCaptureManifest,
    active: Option<&CaptureSet<S>>,
    retained: &[&CaptureSet<S>],
    mut open: impl FnMut(&NodeId, &ApplicationCaptureRequest, Option<&Arc<S>>) -> Result<Arc<S>, E>,
) -> Result<CaptureSet<S>, PrepareError<E>> {
    validate_manifest(manifest)?;
    let mut sources = Vec::with_capacity(manifest.sources.len());
    for source in &manifest.sources {
        let request = &source.request;
        let node_id = NodeId::from(source.node_id.as_str());
        let mut candidates = active
            .into_iter()
            .chain(retained.iter().copied())
            .flat_map(|set| set.sources.iter().map(move |entry| (set.revision, entry)))
            .filter(|(_, entry)| entry.node_id == node_id.as_str());
        // A lost-response reservation can already own the exact successor. Find
        // that owner before considering replacement of an older active session.
        let reusable = candidates
            .clone()
            .find(|(revision, entry)| {
                same_capture_session(&entry.request, request)
                    && (request.source_playback
                        == mixrelay_audio_graph::SourcePlaybackPolicy::Preserve
                        || *revision == manifest.revision)
            })
            .map(|(_, entry)| entry);
        // Scoped mute admission is renewed only by a new explicit manifest.
        // A retry of that same revision retains its already prepared lease.
        let previous = reusable.or_else(|| candidates.next().map(|(_, entry)| entry));
        let session = if let Some(entry) = reusable {
            Arc::clone(&entry.session)
        } else {
            open(&node_id, request, previous.map(|entry| &entry.session))
                .map_err(PrepareError::Open)?
        };
        sources.push(Entry {
            node_id: node_id.as_str().to_owned(),
            request: request.clone(),
            session,
            activate: reusable.map_or(
                previous.is_some()
                    || request.source_playback
                        == mixrelay_audio_graph::SourcePlaybackPolicy::MuteWhileCaptured,
                |entry| entry.activate,
            ),
        });
    }
    Ok(CaptureSet {
        revision: manifest.revision,
        sources,
    })
}

fn validate_manifest(manifest: &ApplicationCaptureManifest) -> Result<(), CaptureError> {
    if manifest.protocol_version != PROTOCOL_VERSION || manifest.sources.len() > 64 {
        return Err(CaptureError::UnsupportedManifest);
    }
    let mut ids = BTreeSet::new();
    for source in &manifest.sources {
        let id = &source.node_id;
        if id.is_empty()
            || id.len() >= 128
            || id.chars().any(char::is_control)
            || !ids.insert(id)
            || source.request.gain_db != 0.0
            || source.request.requires_reselection
            || source.request.stream.channels != 2
            || !matches!(
                &source.request.selector,
                ApplicationSelector::BundleId {
                    include_descendants: false,
                    ..
                } | ApplicationSelector::AllExcept { .. }
            )
        {
            return Err(CaptureError::InvalidIdentity);
        }
        source.request.validate_contract(64)?;
    }
    Ok(())
}

/// Active PCM owners and accepted-but-not-yet-active successors have separate lifetimes.
/// The prior graph may still be rendering after HTTP acceptance.
#[derive(Debug)]
pub struct CaptureOwners<S> {
    active: Option<CaptureSet<S>>,
    pending: Option<CaptureSet<S>>,
    reserved: Option<CaptureSet<S>>,
    retiring: Vec<CaptureSet<S>>,
}
impl<S> Default for CaptureOwners<S> {
    fn default() -> Self {
        Self {
            active: None,
            pending: None,
            reserved: None,
            retiring: Vec::new(),
        }
    }
}
impl<S> CaptureOwners<S> {
    /// Sessions that remain eligible for source reuse.
    /// # Errors
    /// Returns backpressure when too many revisions lack acknowledgement.
    pub fn retained(&self) -> Result<Vec<&CaptureSet<S>>, CaptureError> {
        // Backpressure precedes native preparation and HTTP publication. An
        // unacknowledged engine cannot accumulate unlimited provisional owners.
        if self.retiring.len() >= 64 {
            return Err(CaptureError::UnacknowledgedLimit);
        }
        Ok(self
            .reserved
            .iter()
            .chain(self.active.iter().chain(self.retiring.iter().rev()))
            .collect())
    }
    /// Reserve before sending a mutation, while failure can still safely drop new taps.
    /// # Errors
    /// Returns backpressure or an unresolved prior mutation.
    pub fn ensure_reservable(&self) -> Result<(), CaptureError> {
        if self.reserved.is_some() {
            return Err(CaptureError::UnresolvedMutation);
        }
        if self.retiring.len() >= 64 {
            return Err(CaptureError::UnacknowledgedLimit);
        }
        Ok(())
    }
    /// Reserves a validated successor before submitting its mutation.
    /// # Errors
    /// Returns revision, unresolved mutation, or allocation failures.
    pub fn reserve(&mut self, prepared: CaptureSet<S>) -> Result<(), CaptureError> {
        self.ensure_reservable()?;
        self.validate_successor(&prepared)?;
        self.retiring
            .try_reserve(1)
            .map_err(CaptureError::Reservation)?;
        self.reserved = Some(prepared);
        Ok(())
    }
    /// Reservation already checked revision/capacity before PUT. Publication does
    /// not allocate, revalidate, or discard an accepted owner on a later error.
    /// # Errors
    /// Returns an error when no mutation has been reserved.
    pub fn accept_reserved(&mut self) -> Result<Option<CaptureSet<S>>, CaptureError> {
        let prepared = self
            .reserved
            .take()
            .ok_or(CaptureError::MissingReservation)?;
        Ok(self.publish(prepared))
    }
    pub fn reject_reserved(&mut self) -> Option<CaptureSet<S>> {
        self.reserved.take()
    }
    #[must_use]
    pub fn has_unacknowledged(&self) -> bool {
        self.pending.is_some() || self.reserved.is_some()
    }
    #[must_use]
    pub fn requires_manifest(&self, submitted_revision: u64) -> bool {
        self.reserved.is_some()
            || self.latest().map(|latest| latest.revision) != Some(submitted_revision)
    }
    #[must_use]
    pub fn latest(&self) -> Option<&CaptureSet<S>> {
        self.pending.as_ref().or(self.active.as_ref())
    }
    /// Stages an authoritative manifest without releasing rendering owners.
    /// # Errors
    /// Returns conflicting or stale revisions, or allocation failures.
    pub fn stage(
        &mut self,
        prepared: CaptureSet<S>,
    ) -> Result<Option<CaptureSet<S>>, CaptureError> {
        self.validate_successor(&prepared)?;
        if self
            .reserved
            .as_ref()
            .is_some_and(|reserved| reserved.revision > prepared.revision)
        {
            return Err(CaptureError::UnresolvedMutation);
        }
        self.retiring
            .try_reserve(2)
            .map_err(CaptureError::Reservation)?;
        // A newer authoritative manifest may supersede a lost-response mutation
        // that is currently rendering. Retire that reservation only after its successor ACK.
        self.retiring.extend(self.reserved.take());
        Ok(self.publish(prepared))
    }
    fn validate_successor(&self, prepared: &CaptureSet<S>) -> Result<(), CaptureError> {
        if self
            .latest()
            .is_some_and(|latest| latest.revision > prepared.revision)
        {
            return Err(CaptureError::StaleManifest);
        }
        if let Some(latest) = self
            .latest()
            .filter(|latest| latest.revision == prepared.revision)
            && (latest.sources.len() != prepared.sources.len()
                || latest
                    .sources
                    .iter()
                    .zip(&prepared.sources)
                    .any(|(left, right)| {
                        left.node_id != right.node_id || left.request != right.request
                    }))
        {
            return Err(CaptureError::ConflictingRevision);
        }
        Ok(())
    }
    fn publish(&mut self, prepared: CaptureSet<S>) -> Option<CaptureSet<S>> {
        let revision = prepared.revision;
        let previous = self.pending.replace(prepared);
        if previous
            .as_ref()
            .is_some_and(|previous| previous.revision != revision)
        {
            self.retiring.extend(previous);
            return None;
        }
        previous
    }
    /// Starts every successor before committing handoff; failed starts keep both revisions.
    /// # Errors
    /// Returns the original start failure and any rollback failures without publishing.
    pub fn acknowledge_started<T, E>(
        &mut self,
        active_revision: u64,
        mut start: impl FnMut(&S) -> Result<Option<T>, E>,
        mut rollback: impl FnMut(T) -> Result<(), E>,
        activate: impl FnMut(&S),
    ) -> Result<Vec<CaptureSet<S>>, ActivationError<E>> {
        let mut started = Vec::new();
        if let Some(pending) = self
            .pending
            .as_ref()
            .filter(|set| set.revision <= active_revision)
        {
            for session in pending.sessions() {
                match start(session) {
                    Ok(Some(token)) => started.push(token),
                    Ok(None) => {}
                    Err(start) => {
                        let rollback = started
                            .into_iter()
                            .filter_map(|token| rollback(token).err())
                            .collect();
                        return Err(ActivationError { start, rollback });
                    }
                }
            }
        }
        Ok(self.acknowledge(active_revision, activate))
    }

    fn acknowledge(
        &mut self,
        active_revision: u64,
        activate: impl FnMut(&S),
    ) -> Vec<CaptureSet<S>> {
        if self
            .pending
            .as_ref()
            .is_none_or(|pending| pending.revision > active_revision)
        {
            return Vec::new();
        }
        let Some(prepared) = self.pending.take() else {
            return Vec::new();
        };
        let mut retired = std::mem::take(&mut self.retiring);
        retired.extend(commit(&mut self.active, prepared, activate));
        retired
    }
}

/// The activation callback must be infallible and perform only a control handoff.
/// Return retired owners so their worker joins happen outside the state lock.
fn commit<S>(
    active: &mut Option<CaptureSet<S>>,
    mut prepared: CaptureSet<S>,
    mut activate: impl FnMut(&S),
) -> Option<CaptureSet<S>> {
    for entry in &mut prepared.sources {
        if entry.activate {
            activate(&entry.session);
            entry.activate = false;
        }
    }
    active.replace(prepared)
}

/// Latest committed capture for a non-real-time pump. Only its worker drains the
/// SPSC ring; owners are released on control/pump threads, never an audio callback.
#[derive(Debug)]
pub struct CaptureHandoff<S>(Mutex<Option<S>>);

impl<S> Default for CaptureHandoff<S> {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}

impl<S> CaptureHandoff<S> {
    pub fn publish(&self, capture: S) {
        // No user code runs while locked. Recovering poison cannot violate the
        // single Option invariant, and a successful remote commit cannot fail here.
        let retired = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(capture);
        drop(retired);
    }

    /// For a realtime consumer: never waits for a publisher. A contended handoff stays
    /// published and is taken on a later pass.
    pub fn try_take(&self) -> Option<S> {
        match self.0.try_lock() {
            Ok(mut slot) => slot.take(),
            Err(std::sync::TryLockError::Poisoned(slot)) => slot.into_inner().take(),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }
}

#[cfg(test)]
mod tests;

/// Whether an existing native capture can serve the requested source contract.
#[must_use]
pub fn same_capture_session(
    current: &ApplicationCaptureRequest,
    requested: &ApplicationCaptureRequest,
) -> bool {
    current.selector == requested.selector
        && current.requires_reselection == requested.requires_reselection
        && current.stream == requested.stream
        && current.source_playback == requested.source_playback
        && current.forwarding_endpoint == requested.forwarding_endpoint
}
