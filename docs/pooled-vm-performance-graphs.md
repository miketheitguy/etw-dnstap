# Pooled VM CPU and QPS graphs

These graphs help maintainers compare Windows Performance Recorder (WPR) process
CPU and achieved DNS query rate over time for the four primary five-minute runs. Each graph shows the
management-clean low-rate baseline or a sustained high-rate confirmation on the
same two-vCPU, 4 GiB Windows guest. See the [performance review](pooled-vm-performance-review.md)
for all 22 stages, overload failures, workload details, and lifecycle qualifications.
These observations do not establish universal capacity or a causal improvement
over a pre-pooling build.

This record describes historical binaries with service JSONL metrics
instrumentation. The TCP-only service has no durable metrics export; its
instrumentation and performance have not been measured by this campaign.
See the [instrumentation compatibility note](README.md). Preserved counters,
tables, hashes, and collector artifacts retain their historical meaning.

The four graphs cover two baseline repeats and two high-rate confirmations;
they do not plot all 22 stages or a continuous capacity curve. See
[Measurement terms and result gates](measurement-terms.md) for the shared units
and the difference between delivery, DNS validity, and strict stage outcomes.

## Read the graphs

The upper panel shows one-second scheduled process CPU from native WPR CSwitch
data, excluding observed deferred procedure calls and interrupt service routines
(DPC/ISR). CPU uses one-core units: 100% means one core and a process can exceed
100% on this two-vCPU guest. It is distinct from Performance Data Helper (PDH)
CPU and sampled-profile weights.
Each graph uses its own CPU scale to keep the baseline visible; compare the
numeric axis labels when comparing runs.

The lower panel shows achieved **client sends** per second, estimated by linear
interpolation of each worker's monotone cumulative sent counts at the bin
boundaries. It excludes final drain samples. The dotted line is the requested
rate ceiling. It is not an achieved-rate measurement. RA=false remains a DNS
validation failure even when the send receives a response.

Both panels cover sending seconds 5–295, with one-second bin midpoints joined
by straight lines and no smoothing. The trace mapping uses the same nominal UTC
subtraction as the saved broad native views; guest-host clock offset is not
corrected. The [source manifest](assets/performance/data/source-manifest.json)
records before/after clock bounds and timestamp precision. This approximate
alignment cannot establish subsecond CPU/QPS causality or recover sub-sample
query timing.

The following table retains the full 300-second achieved rates, separate from
the 290-second plotted interior. The [native CPU table](pooled-vm-trace-analysis.md#primary-five-minute-comparisons)
retains independent broad-window CPU totals. All four runs have zero DNS delivery
loss and zero capture loss; the recursive baseline retains its strict failure.

| Run | Requested QPS | Full-sending achieved QPS | Strict stage |
| --- | ---: | ---: | --- |
| Authoritative baseline | 1,000 | 999.70 | PASS |
| Recursive baseline | 1,000 | 999.68 | FAIL: one RA=false response |
| Authoritative high-rate confirmation | 48,000 | 47,303.24 | PASS |
| Recursive high-rate confirmation | 28,000 | 27,758.24 | PASS |

## Authoritative baseline

![Authoritative 1,000 requested QPS baseline: WPR service and Windows DNS CPU above achieved client QPS over sending seconds 5–295.](assets/performance/pooled-auth-1000-300s-2.svg)

[Download SVG](assets/performance/pooled-auth-1000-300s-2.svg) ·
[Plotted numeric CSV](assets/performance/data/pooled-auth-1000-300s-2.csv)

## Recursive baseline

![Recursive 1,000 requested QPS baseline: WPR process CPU and achieved client QPS; strict failure retains one RA=false response with no delivery or capture loss.](assets/performance/pooled-rec-1000-300s-2.svg)

[Download SVG](assets/performance/pooled-rec-1000-300s-2.svg) ·
[Plotted numeric CSV](assets/performance/data/pooled-rec-1000-300s-2.csv)

## Authoritative high-rate confirmation

![Authoritative 48,000 requested QPS confirmation: WPR service and DNS CPU above achieved client QPS; full-sending achieved rate is 47,303.24 QPS.](assets/performance/pooled-auth-48000-300s-1.svg)

[Download SVG](assets/performance/pooled-auth-48000-300s-1.svg) ·
[Plotted numeric CSV](assets/performance/data/pooled-auth-48000-300s-1.csv)

## Recursive high-rate confirmation

![Recursive 28,000 requested QPS confirmation: WPR service and DNS CPU above achieved client QPS; full-sending achieved rate is 27,758.24 QPS.](assets/performance/pooled-rec-28000-300s-1.svg)

[Download SVG](assets/performance/pooled-rec-28000-300s-1.svg) ·
[Plotted numeric CSV](assets/performance/data/pooled-rec-28000-300s-1.csv)

## Reproduce the exports

The [plot renderer](../scripts/performance/plot-pooled-wpr.py) reads the derivative
manifest and verifies each source CSV hash before exporting the four SVGs,
numeric CSV copies, source manifest, and [plot provenance](assets/performance/plot-provenance.json).
SVG text remains selectable; SVGs contain no scripts or external resources.
PNG previews stay under `target/performance/pooled-graph-previews` for visual review.
Rendering uses Python 3.15 and Matplotlib 3.11.2, installed in a task-local virtual
environment. Use a host Python with `venv` and pip's `--python` option available.
Complete these steps from the repository root:

1. Create the task-local virtual environment and install the renderer dependency:

   ```powershell
   python -m venv --without-pip target/performance/graph-tools-venv
   python -m pip --isolated `
     --python target/performance/graph-tools-venv/Scripts/python.exe `
     install --index-url https://pypi.org/simple matplotlib==3.11.2
   ```

   `--isolated` ignores pip environment variables and user configuration,
   including `PIP_NO_INDEX`; the explicit index selects PyPI for this command.
   Use a fresh virtual-environment path if the recorded one already contains
   a different setup.

2. With the preserved campaign evidence and extracted graph manifest available,
   regenerate the exports:

   ```powershell
   target/performance/graph-tools-venv/Scripts/python.exe `
     scripts/performance/plot-pooled-wpr.py `
     target/performance/pooled-vm-20261029-0942/graphs-v1/manifest.json
   ```

3. Confirm that the command produces four SVGs and that the CSV hashes in
   `plot-provenance.json` match their source-manifest identities.

The renderer checks the 290 contiguous bins per run, requested rates, strict
statuses, and SVG XML.
Native extraction and independent integral validation are recorded in the source
manifest; raw campaign evidence remains unchanged.
