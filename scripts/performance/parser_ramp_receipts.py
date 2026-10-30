"""Shared receiver marker fields for campaign-isolated parser-ramp evidence."""


def finish_marker_contract(ready):
    """Return READY contract fields to copy into a final marker when present."""
    keys = ('campaign_id', 'capture_policy', 'expected_operations', 'source_filter_expected')
    return {key: ready[key] for key in keys if key in ready}
