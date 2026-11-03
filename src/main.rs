fn main() -> std::process::ExitCode {
    match etw_dnstap::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        // The service writes nothing to stdout or stderr. Under SCM the failure is
        // already in the Event Log; from a console only the exit code remains.
        Err(_) => std::process::ExitCode::FAILURE,
    }
}
