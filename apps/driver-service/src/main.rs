//! Installed Driver control service executable.

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if std::env::args().skip(1).eq(["--version"]) {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }
    mixrelay_driver_service::logging::init();
    tracing::info!("Driver service starting");
    if let Err(error) = mixrelay_driver_service::run(std::env::args_os().skip(1)).await {
        let detail = format!("{error:#}");
        tracing::error!(error = %detail, "Driver service failed");
        std::process::exit(1);
    }
}
