//! The service controller over simulated `CoreAudio` devices, and the opt-in lab checks
//! against the installed Driver.
mod application_rate;
mod devices;
#[cfg(feature = "native-lab")]
mod installed_driver;
#[cfg(feature = "native-lab")]
mod live_application;
