//! `AudioHardware` status codes returned across the property ABI.
use crate::property_data::DataError;

pub(super) const BAD_OBJECT: i32 = i32::from_be_bytes(*b"!obj");
pub(super) const UNKNOWN: i32 = i32::from_be_bytes(*b"who?");
pub(super) const BAD_SIZE: i32 = i32::from_be_bytes(*b"!siz");
pub(super) const ILLEGAL: i32 = i32::from_be_bytes(*b"nope");
pub(super) const UNSUPPORTED_FORMAT: i32 = i32::from_be_bytes(*b"!dat");

pub(super) fn data_error(error: DataError) -> i32 {
    match error {
        DataError::Size => BAD_SIZE,
        DataError::Type | DataError::Allocation => ILLEGAL,
    }
}
