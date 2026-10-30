//! Send Microsoft DNS Server ETW packet data to a dnstap TCP receiver.

#[cfg(not(target_os = "windows"))]
compile_error!("etw-dnstap requires Windows; build for x86_64-pc-windows-msvc");

mod dnstap;
#[allow(unsafe_code)]
mod etw;
mod framestream;
mod network;
mod pipeline;
#[allow(unsafe_code)]
mod service;

/// Runs capture through the Windows Service Control Manager dispatcher.
///
/// # Errors
/// Returns an error if capture, output, or worker initialization or shutdown fails.
pub fn run() -> anyhow::Result<()> {
    service::run(pipeline::parse_args())
}
