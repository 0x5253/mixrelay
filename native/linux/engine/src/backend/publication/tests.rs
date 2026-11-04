use super::*;
use std::sync::{Arc, Mutex};

/// Records which update was dropped so tests see where ownership ended.
struct Update {
    id: u32,
    drops: Arc<Mutex<Vec<u32>>>,
}

impl Drop for Update {
    fn drop(&mut self) {
        self.drops.lock().unwrap().push(self.id);
    }
}

fn update(id: u32, drops: &Arc<Mutex<Vec<u32>>>) -> Update {
    Update {
        id,
        drops: Arc::clone(drops),
    }
}

#[test]
fn only_one_update_can_wait_for_the_audio_thread() {
    let drops = Arc::default();
    let mailbox = UpdateMailbox::default();
    assert!(!mailbox.has_pending());
    assert!(mailbox.publish(update(1, &drops)).is_ok());
    assert!(mailbox.has_pending());
    let rejected = mailbox.publish(update(2, &drops)).unwrap_err();
    assert_eq!(rejected.id, 2);
    drop(rejected);
    assert_eq!(*drops.lock().unwrap(), [2]);
}

#[test]
fn rejected_transfer_stays_pending_for_the_next_cycle() {
    let drops = Arc::default();
    let mailbox = UpdateMailbox::default();
    assert!(mailbox.publish(update(1, &drops)).is_ok());
    assert_eq!(
        mailbox.activate(|_| Err(Failure::ResourceRevision)),
        Err(Failure::ResourceRevision)
    );
    assert!(mailbox.has_pending());
    let mut offered = None;
    assert_eq!(
        mailbox.activate(|update| {
            offered = Some(update.id);
            Ok(())
        }),
        Ok(())
    );
    assert_eq!(offered, Some(1));
    assert!(drops.lock().unwrap().is_empty());
}

#[test]
fn activation_retires_without_freeing_and_waits_for_reclamation() {
    let drops = Arc::default();
    let mailbox = UpdateMailbox::default();
    assert_eq!(
        mailbox.activate(|_| unreachable!("nothing pending")),
        Ok(())
    );
    assert!(mailbox.publish(update(1, &drops)).is_ok());
    assert_eq!(mailbox.activate(|_| Ok(())), Ok(()));
    assert!(!mailbox.has_pending());
    assert!(drops.lock().unwrap().is_empty());

    assert!(mailbox.publish(update(2, &drops)).is_ok());
    assert_eq!(
        mailbox.activate(|_| unreachable!("retired owners are still live")),
        Err(Failure::RetirementPending)
    );
    mailbox.reclaim_retired();
    assert_eq!(*drops.lock().unwrap(), [1]);
    assert_eq!(mailbox.activate(|_| Ok(())), Ok(()));
}

#[test]
fn dropping_the_mailbox_releases_pending_and_retired_updates() {
    let drops = Arc::default();
    let mailbox = UpdateMailbox::default();
    assert!(mailbox.publish(update(1, &drops)).is_ok());
    assert_eq!(mailbox.activate(|_| Ok(())), Ok(()));
    mailbox.publish_reserved(Box::new(update(2, &drops)));
    drop(mailbox);
    assert_eq!(*drops.lock().unwrap(), [2, 1]);
}

#[test]
fn audio_thread_activation_neither_allocates_nor_frees() {
    let mailbox = UpdateMailbox::default();
    assert!(mailbox.publish(1_u32).is_ok());
    let measured = allocation_counter::measure(|| {
        assert_eq!(mailbox.activate(|_| Ok(())), Ok(()));
    });
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
}
