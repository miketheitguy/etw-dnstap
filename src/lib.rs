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
/// Help and version output print to stdout and succeed without starting the
/// dispatcher. Other argument errors are passed to the service layer, which
/// reports them through the Event Log and SCM, or returns them to a console user.
///
/// # Errors
/// Returns an error if capture, output, or worker initialization or shutdown fails.
pub fn run() -> anyhow::Result<()> {
    match pipeline::parse_args() {
        Ok(args) => service::run(Ok(args)),
        Err(error) if is_informational(error.kind()) => {
            error.print()?;
            Ok(())
        }
        Err(error) => {
            let rendered = error.render().to_string();
            service::run(Err(strip_clap_error_prefix(&rendered).to_owned()))
        }
    }
}

/// Help and version requests are successful output, not argument failures.
fn is_informational(kind: clap::error::ErrorKind) -> bool {
    matches!(
        kind,
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
    )
}

/// Removes clap's leading `error: ` label once, because the service layer adds its
/// own label and the text would otherwise repeat it.
fn strip_clap_error_prefix(rendered: &str) -> &str {
    rendered.strip_prefix("error: ").unwrap_or(rendered)
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;

    use super::{is_informational, strip_clap_error_prefix};

    #[test]
    fn only_help_and_version_are_informational() {
        assert!(is_informational(ErrorKind::DisplayHelp));
        assert!(is_informational(ErrorKind::DisplayVersion));
        assert!(!is_informational(ErrorKind::MissingRequiredArgument));
        assert!(!is_informational(ErrorKind::InvalidValue));
    }

    #[test]
    fn strips_only_one_leading_error_label() {
        assert_eq!(
            strip_clap_error_prefix("error: missing --receiver"),
            "missing --receiver"
        );
        assert_eq!(
            strip_clap_error_prefix("error: error: nested"),
            "error: nested"
        );
        assert_eq!(strip_clap_error_prefix("Error: kept"), "Error: kept");
        assert_eq!(strip_clap_error_prefix("usage: kept"), "usage: kept");
        assert_eq!(strip_clap_error_prefix(""), "");
    }
}
