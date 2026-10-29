"""Validate service metrics and normalize schema-specific admission losses."""

LEDGER_COUNTERS = (
    "received", "ring_dropped", "callback_errors", "dequeued", "queue_backlog",
    "encoded", "ignored", "rejected", "output_limit_dropped",
    "output_failed_events", "output_errors", "etw_events_lost",
    "etw_realtime_buffers_lost", "etw_stats_errors",
)
INTAKE_COUNTERS = ("intake_dropped", "pool_exhausted", "receiver_disconnected")


def nonnegative_integer(sample, key):
    """Return an explicitly supplied nonnegative integer, rejecting booleans."""
    value = sample.get(key)
    if type(value) is not int or value < 0:
        raise ValueError(f"Metrics {key} must be an explicit nonnegative integer")
    return value


def normalize_metrics(sample, *, require_final=True):
    """Return a normalized copy and enforce closure for explicitly final samples.

    Live samples read separate relaxed atomics, so their residuals and the
    schema 2 intake category sum need not agree at a single instant. Setting
    require_final=False permits live samples without weakening final samples.
    """
    schema = sample.get("schema")
    if type(schema) is not int or schema not in (1, 2):
        raise ValueError(f"Unsupported service metrics schema: {schema!r}")
    if type(sample.get("final")) is not bool:
        raise ValueError("Metrics final must be an explicit Boolean")
    if require_final and sample["final"] is not True:
        raise ValueError("Service metrics are not final")
    if type(sample.get("etw_stats_valid")) is not bool:
        raise ValueError("Metrics etw_stats_valid must be an explicit Boolean")
    normalized = dict(sample)
    for key in LEDGER_COUNTERS:
        normalized[key] = nonnegative_integer(sample, key)
    if schema == 2:
        for key in INTAKE_COUNTERS:
            normalized[key] = nonnegative_integer(sample, key)
    else:
        normalized.update(intake_dropped=normalized["ring_dropped"],
                          pool_exhausted=0, receiver_disconnected=0)
    ledger = ledger_summary(normalized)
    if sample["final"] and any(ledger[key] for key in (
            "intake_residual", "capture_residual", "parser_residual")):
        raise ValueError(f"Final service ledger does not reconcile: {ledger}")
    return normalized


def ledger_summary(sample):
    """Return residuals for an already normalized sample."""
    return {
        "final_sample": sample["final"],
        "schema": sample["schema"],
        "intake_dropped": sample["intake_dropped"],
        "ring_dropped": sample["ring_dropped"],
        "pool_exhausted": sample["pool_exhausted"],
        "receiver_disconnected": sample["receiver_disconnected"],
        "intake_residual": sample["intake_dropped"] - sum(sample[key] for key in
            ("ring_dropped", "pool_exhausted", "receiver_disconnected")),
        "capture_residual": sample["received"] - sum(sample[key] for key in
            ("intake_dropped", "callback_errors", "dequeued", "queue_backlog")),
        "parser_residual": sample["dequeued"] - sum(sample[key] for key in
            ("encoded", "ignored", "rejected", "output_limit_dropped", "output_failed_events")),
    }


def validate_sample_pair(previous, current, keys):
    """Reject mixed capture identities and counter resets in normalized samples."""
    for key in ("schema", "run_id", "pid", "session"):
        if previous.get(key) != current.get(key):
            raise ValueError(f"Metrics identity changed: {key}")
    for key in keys:
        if nonnegative_integer(current, key) < nonnegative_integer(previous, key):
            raise ValueError(f"Counter reset: {key}")
