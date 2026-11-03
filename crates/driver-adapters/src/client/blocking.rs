//! One lazy blocking HTTP runtime and connection pool per Driver client and its clones.

use std::{path::Path, time::Duration};

use parking_lot::Mutex;

use mixrelay_driver_core::DriverError;

#[derive(Debug, Default)]
pub(super) struct Transport {
    client: Mutex<Option<reqwest::blocking::Client>>,
}

impl Transport {
    // Construct only on the synchronous caller's control thread. Async clients
    // need no blocking runtime; requests release this lock before socket I/O.
    pub(super) fn client(
        &self,
        socket_path: &Path,
    ) -> Result<reqwest::blocking::Client, DriverError> {
        let mut cached = self.client.lock();
        if let Some(client) = cached.as_ref() {
            return Ok(client.clone());
        }
        let client = reqwest::blocking::Client::builder()
            .unix_socket(socket_path.to_path_buf())
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(super::transport_error)?;
        *cached = Some(client.clone());
        Ok(client)
    }
}
