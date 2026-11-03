use super::super::native::StreamIds;
use super::*;
use std::{cell::Cell, ffi::CStr};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    Set,
    Get,
    Ignored,
    Truncated,
    Count,
    Identity,
    Topology,
}

struct Fixture {
    count: usize,
    fault: Fault,
    written: Cell<Option<(u32, usize)>>,
}

impl Properties for Fixture {
    fn read<T: Copy + Default>(&self, _: u32, _: Address) -> Option<T> {
        None
    }
    fn device(&self, _: &CStr) -> Option<u32> {
        None
    }
    fn streams(&self, _: u32, _: u32, _: u32) -> Option<StreamIds> {
        let changed = self.written.get().is_some() && self.fault == Fault::Topology;
        Some(StreamIds {
            count: self.count,
            values: std::array::from_fn(|index| {
                u32::try_from(index).unwrap() + 1 + u32::from(changed)
            }),
        })
    }
}

impl Backend for Fixture {
    fn write_usage(&self, _: u32, scope: u32, usage: &Usage, bytes: u32) -> i32 {
        assert_eq!(usage.count as usize, self.count);
        assert_eq!(bytes as usize, 12 + self.count * 4);
        assert!(usage.enabled[..self.count].iter().all(|&on| on == 0));
        self.written.set(Some((scope, usage.procedure.addr())));
        if self.fault == Fault::Set { -17 } else { 0 }
    }
    fn read_usage(&self, _: u32, scope: u32, usage: &mut Usage, bytes: &mut u32) -> i32 {
        assert_eq!(self.written.get(), Some((scope, usage.procedure.addr())));
        match self.fault {
            Fault::Get => return -18,
            Fault::Truncated => *bytes -= 4,
            Fault::Count => usage.count += 1,
            Fault::Identity => usage.procedure = ptr::null_mut(),
            _ => (),
        }
        if self.fault != Fault::Ignored {
            usage.enabled.fill(0);
        }
        0
    }
}

#[test]
fn all_unused_streams_are_disabled_only_for_the_selected_registration_and_direction() {
    for input in [false, true] {
        for count in [0, 1, 3, MAX_STREAMS] {
            let fixture = Fixture {
                count,
                fault: Fault::None,
                written: Cell::new(None),
            };
            let procedure = NonNull::new(ptr::without_provenance_mut(93)).unwrap();
            assert_eq!(disable_unused(&fixture, 7, procedure, input, 2048), 0);
            assert_eq!(
                fixture.written.get(),
                (count != 0).then_some((
                    u32::from_be_bytes(if input { *b"outp" } else { *b"inpt" }),
                    93,
                ))
            );
        }
    }
}

#[test]
fn failed_ignored_or_racing_configuration_cannot_start_io() {
    for fault in [
        Fault::Set,
        Fault::Get,
        Fault::Ignored,
        Fault::Truncated,
        Fault::Count,
        Fault::Identity,
        Fault::Topology,
    ] {
        let fixture = Fixture {
            count: 3,
            fault,
            written: Cell::new(None),
        };
        assert_ne!(
            disable_unused(&fixture, 7, NonNull::dangling(), false, 64),
            0
        );
    }
}
