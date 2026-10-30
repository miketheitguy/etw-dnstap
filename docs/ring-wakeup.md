# Rejected wake-on-enqueue experiment

This historical note records the rejected `wake-1024-v1` candidate for
maintainers reviewing the ring-drop investigation. The active implementation
uses timed polling; see [Ring capacity and shutdown](ring-buffer.md).

This record describes historical binaries with service JSONL metrics
instrumentation. The TCP-only service has no durable metrics export; its
instrumentation and performance have not been measured by this campaign.
See the [instrumentation compatibility note](README.md). Preserved counters,
tables, hashes, and collector artifacts retain their historical meaning.

The experiment retained the 1024-event ring and 2 ms idle timeout. It added an
owned parser `Thread` handle to the callback context and called `unpark` after
every successful enqueue. Synthetic wake and ownership tests passed, but a
30-second authoritative pilot at approximately 1,000 queries per second dropped
24,342 of 59,938 received events. No ETW loss or callback, parser, or output error
explained those drops. The receiver decoded all 35,596 emitted events. These are
missing capture records; they do not establish lost DNS queries or responses.

The production wake changes and wake-specific tests were reverted together.
The preserved source snapshot, binary, and local validation receipts remain
under `target/performance/ring-fix-20261029/candidate/wake-1024-v1` and
`target/performance/ring-fix-20261029/code-gate`. Passing those synthetic tests
did not establish sufficient capacity for native delivery bursts.
