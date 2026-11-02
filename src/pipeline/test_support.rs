//! Helpers shared by the tests of several pipeline modules.

use super::config::{PipelineConfig, parse_args_from};

/// A validated configuration for a loopback receiver with `extra` arguments.
pub(super) fn config_from(extra: &[&str]) -> PipelineConfig {
    let mut arguments = vec!["etw-dnstap", "--receiver", "127.0.0.1:6000"];
    arguments.extend_from_slice(extra);
    PipelineConfig::try_from(parse_args_from(arguments).expect("valid arguments"))
        .expect("valid configuration")
}
