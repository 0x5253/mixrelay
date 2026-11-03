//! Verify the actual subscription; ALSA source addresses can be reused between scans.
use alsa::seq::{Addr, PortSubscribeIter, QuerySubsType, Seq};

pub(super) fn subscribed(seq: &Seq, source: &str, receiver: &str) -> bool {
    let Ok(source) = source.parse::<Addr>() else {
        return false;
    };
    PortSubscribeIter::new(seq, source, QuerySubsType::READ).any(|subscription| {
        seq.get_any_client_info(subscription.get_dest().client)
            .is_ok_and(|client| client.get_name().is_ok_and(|name| name == receiver))
    })
}
