"""Check evidence rejection; these fixtures do not simulate the Rust callback."""

import copy
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))  # `python -I` does not add the script directory

import pooled_intake_run as RUNNER  # noqa: E402


def fixture():
    return dict(capacity=1024, mode="steady", offered=100, accepted=90, dequeued=90,
                intake_dropped=10, ring_dropped=5, pool_exhausted=3, receiver_disconnected=2,
                callback_errors=0, final_backlog=0, elapsed_ns=1000, recycle_verified=True,
                allocator_fixtures={name: dict(allocations=0, deallocations=0, reallocations=0)
                    for name in ("success", "channel_full", "pool_exhausted", "disconnected")})


class ValidationTests(unittest.TestCase):
    def test_valid_lossy_evidence_is_not_falsely_zero_loss(self):
        self.assertEqual(RUNNER.validate(fixture(), 1024, "steady")["intake_dropped"], 10)

    def test_ledger_residual_rejected(self):
        for key in ("offered", "accepted", "dequeued", "intake_dropped", "pool_exhausted"):
            with self.subTest(key=key):
                value = fixture()
                value[key] += 1
                with self.assertRaises(ValueError):
                    RUNNER.validate(value, 1024, "steady")

    def test_missing_allocator_evidence_rejected(self):
        value = fixture()
        del value["allocator_fixtures"]["disconnected"]
        with self.assertRaises(KeyError):
            RUNNER.validate(value, 1024, "steady")

    def test_each_allocator_operation_rejected(self):
        for name, counts in fixture()["allocator_fixtures"].items():
            for operation in counts:
                with self.subTest(name=name, operation=operation):
                    value = copy.deepcopy(fixture())
                    value["allocator_fixtures"][name][operation] = 1
                    with self.assertRaises(ValueError):
                        RUNNER.validate(value, 1024, "steady")

    def test_wrong_matrix_and_ownership_rejected(self):
        with self.assertRaises(ValueError):
            RUNNER.validate(fixture(), 8192, "steady")
        with self.assertRaises(ValueError):
            RUNNER.validate(fixture(), 1024, "bursty")
        value = fixture()
        value["recycle_verified"] = False
        with self.assertRaises(ValueError):
            RUNNER.validate(value, 1024, "steady")


if __name__ == "__main__":
    unittest.main()
