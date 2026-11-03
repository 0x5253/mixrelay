//! HAL reports to the Driver service. Both ends use this one codec, so every field the
//! HAL writes is the field the service reads. Both ship in one package; a report of
//! another version means a partial install and decodes as `Incompatible` instead of
//! being ignored.
use super::{
    Message, Xpc, dictionary, endpoint_id, field, string, uint, xpc_dictionary_create,
    xpc_dictionary_set_data, xpc_dictionary_set_string, xpc_dictionary_set_uint64,
};
use std::{ffi::CStr, ptr};

/// The report wire version.
pub const VERSION: u64 = 2;

const INVENTORY: &CStr = c"inventory_ack";
const SETTLED: &CStr = c"configuration_settled";
const REPLAYED: &CStr = c"configuration_replayed";
const UNPROVISIONED: &CStr = c"clients_unprovisioned";

/// One HAL report on the control connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    /// The HAL finished one inventory generation.
    Inventory { generation: u64, applied: bool },
    /// How one rate, format or mapping request ended.
    Settled(Settled),
    /// Every retained settlement was resent on this connection.
    Replayed { incarnation: u64, through: u64 },
    /// Clients holding IO on `endpoint` that have no queue; 0 reports recovery.
    Unprovisioned { endpoint: [u8; 16], clients: u32 },
}

/// How the HAL ended one rate, format or mapping request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settled {
    /// The HAL load that settled it; sequences restart with each one.
    pub incarnation: u64,
    pub endpoint: [u8; 16],
    /// Settlement order within the incarnation.
    pub sequence: u64,
    pub rate: u32,
    /// Requested capture and render `SampleFormat` codes.
    pub formats: [u32; 2],
    pub applied: bool,
}

/// A report written by a HAL of another package version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Incompatible;

impl Report {
    /// A new message holding one reference for the caller, or null when XPC could not
    /// allocate one.
    #[must_use]
    pub fn encode(&self) -> Xpc {
        let (operation, endpoint, fields): (_, _, &[(&CStr, u64)]) = match *self {
            Self::Inventory {
                generation,
                applied,
            } => (
                INVENTORY,
                None,
                &[
                    (c"generation", generation),
                    (c"applied", u64::from(applied)),
                ],
            ),
            Self::Settled(settled) => (
                SETTLED,
                Some(settled.endpoint),
                &[
                    (c"incarnation", settled.incarnation),
                    (c"sequence", settled.sequence),
                    (c"rate", u64::from(settled.rate)),
                    (c"capture_format", u64::from(settled.formats[0])),
                    (c"render_format", u64::from(settled.formats[1])),
                    (c"applied", u64::from(settled.applied)),
                ],
            ),
            Self::Replayed {
                incarnation,
                through,
            } => (
                REPLAYED,
                None,
                &[(c"incarnation", incarnation), (c"through", through)],
            ),
            Self::Unprovisioned { endpoint, clients } => (
                UNPROVISIONED,
                Some(endpoint),
                &[(c"clients", u64::from(clients))],
            ),
        };
        // SAFETY: XPC copies every key, string and data extent into the new dictionary.
        let Some(message) =
            Message::new(unsafe { xpc_dictionary_create(ptr::null(), ptr::null(), 0) })
        else {
            return ptr::null_mut();
        };
        unsafe {
            xpc_dictionary_set_uint64(message.0, c"version".as_ptr(), VERSION);
            xpc_dictionary_set_string(message.0, c"operation".as_ptr(), operation.as_ptr());
            if let Some(endpoint) = endpoint {
                xpc_dictionary_set_data(
                    message.0,
                    c"endpoint_id".as_ptr(),
                    endpoint.as_ptr().cast(),
                    endpoint.len(),
                );
            }
            for (key, value) in fields {
                xpc_dictionary_set_uint64(message.0, key.as_ptr(), *value);
            }
        }
        message.into_raw()
    }

    /// Reads a HAL report: `None` for any other or malformed message, `Incompatible`
    /// for a report of another version.
    /// # Safety
    /// `message` is null or a live XPC object borrowed for this call.
    pub unsafe fn decode(message: Xpc) -> Option<Result<Self, Incompatible>> {
        if !dictionary(message) {
            return None;
        }
        let operation = string(field(message, c"operation"))?;
        let operation = operation.as_c_str();
        if ![INVENTORY, SETTLED, REPLAYED, UNPROVISIONED].contains(&operation) {
            return None;
        }
        if uint(message, c"version") != Some(VERSION) {
            return Some(Err(Incompatible));
        }
        let word = |key| uint(message, key);
        let small = |key| word(key).and_then(|value| u32::try_from(value).ok());
        let flag = |key| match word(key)? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
        let report = if operation == INVENTORY {
            Self::Inventory {
                generation: word(c"generation")?,
                applied: flag(c"applied")?,
            }
        } else if operation == SETTLED {
            Self::Settled(Settled {
                incarnation: word(c"incarnation")?,
                endpoint: endpoint_id(message)?,
                sequence: word(c"sequence")?,
                rate: small(c"rate")?,
                formats: [small(c"capture_format")?, small(c"render_format")?],
                applied: flag(c"applied")?,
            })
        } else if operation == REPLAYED {
            Self::Replayed {
                incarnation: word(c"incarnation")?,
                through: word(c"through")?,
            }
        } else {
            Self::Unprovisioned {
                endpoint: endpoint_id(message)?,
                clients: small(c"clients")?,
            }
        };
        Some(Ok(report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(report: Report) -> Option<Result<Report, Incompatible>> {
        let message = Message::new(report.encode()).unwrap();
        // SAFETY: the test owns the live message for this call.
        unsafe { Report::decode(message.0) }
    }

    #[test]
    fn every_hal_report_reads_back_as_written() {
        let mut endpoint = [1; 16];
        endpoint[8..].fill(2);
        for report in [
            Report::Inventory {
                generation: 9,
                applied: true,
            },
            Report::Inventory {
                generation: 10,
                applied: false,
            },
            Report::Settled(Settled {
                incarnation: u64::MAX,
                endpoint,
                sequence: 12,
                rate: 96_000,
                formats: [1, 3],
                applied: false,
            }),
            Report::Replayed {
                incarnation: 7,
                through: 3,
            },
            Report::Unprovisioned {
                endpoint,
                clients: 2,
            },
        ] {
            assert_eq!(round_trip(report), Some(Ok(report)));
        }
    }

    #[test]
    fn reports_of_another_version_are_surfaced_and_other_messages_ignored() {
        for operation in [INVENTORY, SETTLED, REPLAYED, UNPROVISIONED] {
            for version in [1, 3] {
                // An older HAL's report used to be dropped, leaving every change to
                // wait out its whole budget and report pending.
                let message =
                    Message::new(unsafe { xpc_dictionary_create(ptr::null(), ptr::null(), 0) })
                        .unwrap();
                unsafe {
                    xpc_dictionary_set_uint64(message.0, c"version".as_ptr(), version);
                    xpc_dictionary_set_string(message.0, c"operation".as_ptr(), operation.as_ptr());
                }
                assert_eq!(
                    unsafe { Report::decode(message.0) },
                    Some(Err(Incompatible))
                );
            }
        }
        let request =
            Message::new(super::super::mr_hal_control_create_inventory_request()).unwrap();
        assert_eq!(unsafe { Report::decode(request.0) }, None);
    }
}
