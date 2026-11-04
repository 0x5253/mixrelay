//! Control-side producer registration, replacement and retirement. Attach and
//! collection share the control mutex; callbacks never reach this module.
use super::{Attachment, Control, SourceRegistry, identity::SourceIdentity, source::Source};
use crate::source_control::SourceRequest;
use std::{io, ptr, sync::atomic::Ordering};

/// Retired owners still pinned by readers; beyond this, attach waits for them.
pub(super) const MAX_RETAINED: usize = 128;

impl SourceRegistry {
    /// # Errors
    /// Invalid geometry/identity, exhausted leases, busy producers, allocation
    /// failure, or bounded retirement pressure reject without publishing state.
    pub fn attach(&self, request: &SourceRequest) -> io::Result<Attachment> {
        let identity = SourceIdentity::validate(request)?;
        let mut control = self.control();
        self.collect(&mut control);
        let lease = control.leases.peek()?;
        let (index, existing) = self.place(&identity)?;
        let old_producer = existing
            .map(|source| {
                source.claim_producer().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::WouldBlock, "source producer busy")
                })
            })
            .transpose()?;
        // A live mapping is leased again in place; a new or exhausted one is replaced.
        let attachment = match existing.filter(|source| !source.terminal()) {
            Some(live) => live.grant(lease, None)?,
            None => self.publish_replacement(&mut control, index, identity, (lease, existing))?,
        };
        control.leases.spend();
        drop(old_producer);
        self.collect(&mut control);
        Ok(attachment)
    }

    /// The slot a request lands in, with the source it already names there.
    fn place(&self, identity: &SourceIdentity) -> io::Result<(usize, Option<&Source>)> {
        let existing = self.slots.find(identity.token);
        if existing.is_some_and(|(_, source)| !source.matches(identity)) {
            return Err(io::Error::other("source identity/geometry conflict"));
        }
        let index = existing
            .map(|(index, _)| index)
            .or_else(|| self.slots.vacant())
            .ok_or_else(|| io::Error::other("source inventory full"))?;
        Ok((index, existing.map(|(_, source)| source)))
    }

    /// Allocates and grants a new mapping, then publishes it in `index`,
    /// taking over from `previous` when it replaces an exhausted source.
    fn publish_replacement(
        &self,
        control: &mut Control,
        index: usize,
        identity: SourceIdentity,
        (lease, previous): (u64, Option<&Source>),
    ) -> io::Result<Attachment> {
        if control.owners.len() >= MAX_RETAINED {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "source readers must drain",
            ));
        }
        let mapping = self.mapping(identity.capacity, identity.channels)?;
        let candidate = Box::pin(Source::new(identity, mapping));
        let attachment = candidate.grant(lease, previous)?;
        // The pin remains control-owned. Readers access only immutable
        // fields, atomics and the independently claimed timeline cell.
        let published = ptr::from_ref(candidate.as_ref().get_ref());
        control.owners.push(candidate);
        self.slots.publish(index, published);
        Ok(attachment)
    }

    /// Closes only the local grant whose attachment failed; a later lease survives.
    pub(crate) fn cancel_attachment(&self, lease: u64) {
        let mut control = self.control();
        // The control mutex excludes grant replacement while this failed local
        // attachment closes.
        for (_, source) in self.slots.published() {
            // AcqRel revokes exactly the failed lease; it cannot close a newer producer.
            let _ = source.mapping.clock().producer_lease.compare_exchange(
                lease,
                0,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        self.collect(&mut control);
    }

    /// Reclaims closed producers on control after their endpoint workers have joined.
    /// Existing graph pins retain mappings until a later collection observes no readers.
    pub fn collect_retired(&self) {
        self.collect(&mut self.control());
    }

    /// A zero lease means its unique producer closed, not that an active
    /// producer is merely paused. Unpublish first; pinned owners stay retained.
    pub(super) fn collect(&self, control: &mut Control) {
        self.slots.unpublish_closed();
        if self.slots.drained() {
            control
                .owners
                .retain(|owner| self.slots.contains(owner.as_ref().get_ref()));
        }
    }
}
