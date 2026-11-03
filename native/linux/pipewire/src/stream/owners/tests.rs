use super::*;

#[test]
fn independent_scopes_and_cloned_handles_count_until_the_last_allocation_token_retires() {
    let a = StreamOwners::default();
    let b = StreamOwners::default();
    let owner = a.acquire();
    let second = a.clone().acquire();
    let unrelated = b.acquire();
    assert_eq!(a.active_owners(), 2);
    assert_eq!(b.active_owners(), 1);
    drop(owner);
    assert_eq!(a.active_owners(), 1);
    drop(unrelated);
    assert_eq!(b.active_owners(), 0);
    assert_eq!(a.active_owners(), 1);
    drop(second);
    assert_eq!(a.active_owners(), 0);
}
