#!/usr/bin/env python3
"""Summarize native CSVs and xctrace Metal exports without conflating pacing with work.

ROOT contains validated-MODE/{metadata.json,frames.csv} and
gpu-MODE/{metadata.json,frames.csv,metal-toc.xml,metal-data.xml}.
The latter XML exports select displayed-surfaces-interval and metal-gpu-intervals
from a Metal System Trace recording. An optional focused-window folder holds
an additional foreground validation capture in the same format.
"""

import argparse
import bisect
import csv
import gzip
import datetime
import json
import math
import re
import statistics
import xml.etree.ElementTree as ET
from collections import defaultdict
from pathlib import Path

BUDGET_MS = 1000 / 120


def sample_scanout_intervals(surfaces, vsyncs):
    """Retained scanout buffers may be rewritten without a surface-ID change.

    Sample their resource history at real hardware pulses inside each visible
    interval. This does not invent app updates: resolve_scanout must still
    establish the sampled app version, and repeated versions are collapsed.
    """
    result = []
    for event in surfaces:
        pulses = vsyncs.get(event.get("display"), [])
        end = event["at"] + event["duration_ms"] / 1000
        inside = pulses[bisect.bisect_right(pulses, event["at"] + .00025):
                        bisect.bisect_left(pulses, end - .00025)]
        if not inside:
            result.append(event)
            continue
        times = [event["at"], *inside, end]
        for at, stop in zip(times, times[1:]):
            result.append(dict(event, at=at, duration_ms=1000 * (stop - at),
                               # The interval's original latency belongs to
                               # its original presentation, not later rewrites.
                               latency_ms=event.get("latency_ms") if at == event["at"] else None))
    return result


def resolve_scanout(surfaces, app_writes, compositor_frames):
    """Follow the app texture version sampled into each scanout buffer.

    Surface numbers in narrative Frame labels and GPU Frame counters are
    separate namespaces. Join by the texture resource and GPU ordering instead.
    Freeze the source version at the compositor read, not at scanout: the
    client's drawable may have been reused by then.
    """
    history = defaultdict(list)
    app_surfaces = {surface for surface, _, _ in app_writes}
    for surface, end, native in app_writes:
        history[surface].append((end, native))
    for values in history.values():
        values.sort()

    def read(surface, at):
        values = history[surface]
        index = bisect.bisect_right(values, (at, math.inf)) - 1
        return values[index][1] if index >= 0 else -1

    ambiguous = 0
    for group in sorted(compositor_frames, key=lambda g: max(g["writes"].values(), default=0)):
        versions = {read(s, at) for s, at in group["reads"].items() if s in app_surfaces}
        versions.discard(-1)
        if not versions:
            versions = {read(s, at) for s, at in group["reads"].items()}
            versions.discard(-1)
        if len(versions) > 1:
            ambiguous += 1
        native = next(iter(versions)) if len(versions) == 1 else -1
        for surface, end in group["writes"].items():
            bisect.insort(history[surface], (end, native))
    result = []
    for event in sorted(surfaces, key=lambda e: e["at"]):
        native = read(event["surface"], event["at"])
        if native >= 0:
            result.append(dict(event, native_frame=native))
    return result, ambiguous


def distribution(values):
    values = sorted(values)
    if not values:
        return None
    def percentile(p):
        return values[max(0, math.ceil(len(values) * p) - 1)]
    return dict(count=len(values), mean=sum(values) / len(values),
                p50=percentile(.5), p95=percentile(.95),
                p99=percentile(.99), maximum=values[-1])


def cpu_case(folder):
    metadata = json.loads((folder / "metadata.json").read_text())
    with (folder / "frames.csv").open() as stream:
        rows = list(csv.DictReader(stream))
    result = {"metadata": metadata, "phases": {}}
    for phase in ("scrolling", "typing"):
        selected = [r for r in rows if r["phase"] == phase]
        result["phases"][phase] = {
            "submitted_frames": len(selected),
            "submitted_fps": len(selected) / metadata["duration_per_phase_seconds"],
            "cpu_work_ms": distribution([float(r["cpu_work_ms"]) for r in selected]),
            "cpu_total_with_acquire_wait_ms": distribution([float(r["cpu_total_ms"]) for r in selected]),
            "cpu_encode_submit_ms": distribution([float(r["render_submit_ms"]) for r in selected]),
            "post_submit_maintenance_ms": distribution([float(r.get("maintenance_ms", 0)) for r in selected]),
            "acquire_wait_ms": distribution([float(r["acquire_wait_ms"]) for r in selected]),
            "cpu_work_over_budget": sum(float(r["cpu_work_ms"]) > BUDGET_MS for r in selected),
        }
    return result, rows


def union_ms(intervals):
    end = None
    total = 0
    for start, stop in sorted(intervals):
        total += max(0, stop - max(start, end if end is not None else start))
        end = max(stop, end if end is not None else stop)
    return total / 1e6


def presentation_cadence(shown, vsyncs=None, reported_hz=None):
    shown = sorted(shown, key=lambda s: s["at"])
    hardware_hz = None
    if shown and vsyncs is not None and len({s.get("display") for s in shown}) == 1:
        pulses = vsyncs.get(shown[0].get("display"), [])
        first = bisect.bisect_left(pulses, shown[0]["at"] - .00025)
        last = bisect.bisect_right(pulses, shown[-1]["at"] + .00025)
        matched = pulses[first:last]
        if len(matched) >= 4:
            period = statistics.median(b - a for a, b in zip(matched, matched[1:]))
            if period > 0 and matched[0] <= shown[0]["at"] + 2 * period and matched[-1] >= shown[-1]["at"] - 2 * period:
                hardware_hz = 1 / period
    refresh_hz = hardware_hz or reported_hz or 120
    # Visible interval lengths alone hide gaps with no owned surface.
    intervals = [1000 * (b["at"] - a["at"]) for a, b in zip(shown, shown[1:])]
    # Round from a common origin so a consistently slow cadence cannot
    # discard fractional missed slots separately in each interval.
    slots = [round((s["at"] - shown[0]["at"]) * refresh_hz) for s in shown]
    missed = [max(0, b - a - 1) for a, b in zip(slots, slots[1:])]
    misses = sum(missed)
    events = []
    for a, b, t, count in zip(shown, shown[1:], intervals, missed):
        if count == 0:
            continue
        pulses = vsyncs.get(b.get("display")) if vsyncs is not None else None
        # Exclude the pulses at either presentation boundary, allowing for
        # timestamp jitter between the GPU and display instruments.
        between = (pulses[bisect.bisect_right(pulses, a["at"] + .00025):
                          bisect.bisect_left(pulses, b["at"] - .00025)]
                   if pulses is not None and a.get("display") == b.get("display") else None)
        events.append(dict(at_seconds=b["at"], interval_ms=t,
                           missed_slots=count,
                           previous_surface_frame=a.get("frame", a.get("native_frame")),
                           surface_frame=b.get("frame", b.get("native_frame")),
                           intervening_hardware_vsync_seconds=between))
    return dict(displayed_fps=len(intervals) / (sum(intervals) / 1000) if intervals and sum(intervals) > 0 else None,
                cadence_refresh_hz=refresh_hz,
                hardware_refresh_hz=hardware_hz,
                displayed_frame_interval_ms=distribution(intervals),
                missed_presentations=events,
                missed_refresh_slots=misses,
                missed_refresh_percent=100 * misses / (len(intervals) + misses) if intervals else None)


def metal_case(folder):
    cpu, csv_rows = cpu_case(folder)
    toc = ET.parse(folder / "metal-toc.xml").getroot().find("run")
    pid = toc.find("info/target/process").get("pid")
    start = datetime.datetime.fromisoformat(toc.findtext("info/summary/start-date")).timestamp()
    schemas = [t.get("schema") for t in toc.find("data").findall("table")]
    source = folder / "metal-data.xml"
    if source.exists():
        root = ET.parse(source).getroot()
    else:
        with gzip.open(folder / "metal-data.xml.gz", "rb") as stream:
            root = ET.parse(stream).getroot()
    ids = {e.get("id"): e for e in root.iter() if e.get("id")}

    def resolve(e):
        return ids[e.get("ref")] if e.get("ref") else e

    def value(e):
        return resolve(e).text or ""

    def fmt(e):
        return resolve(e).get("fmt", "")

    def process(e):
        p = resolve(e).find("pid")
        return value(p) if p is not None else None

    tables = {}
    for node in root.findall("node"):
        index = int(re.search(r"table\[(\d+)\]", node.get("xpath")).group(1))
        tables[schemas[index - 1]] = node.findall("row")

    vsyncs = defaultdict(list)
    for row in tables.get("display-vsyncs-interval", []):
        if value(row[5]) == "VSYNC":
            vsyncs[fmt(row[2])].append(int(value(row[0])) / 1e9)
    for pulses in vsyncs.values():
        pulses.sort()

    gpu = defaultdict(list)
    app_writes = []
    compositor = defaultdict(lambda: dict(reads={}, writes={}))
    resources = re.compile(r"(Read|Write) Surface: (\d+)\(")
    main_window = cpu["metadata"].get("native_window_id", 0)
    for row in tables.get("metal-gpu-intervals", []):
        at, duration = int(value(row[0])), int(value(row[1]))
        accesses = resources.findall(fmt(row[13])) if len(row) > 13 else []
        if "WindowServer" in fmt(row[10]):
            # A GPU frame can contain several independent compositor command
            # buffers sampling different app versions. Keep their writes apart.
            command = value(row[15]) if len(row) > 15 else ""
            if not command:
                continue
            group = compositor[command]
            for op, surface in accesses:
                target = group["reads" if op == "Read" else "writes"]
                surface = int(surface)
                target[surface] = (min(target.get(surface, math.inf), at / 1e9) if op == "Read"
                                   else max(target.get(surface, 0), (at + duration) / 1e9))
        if process(row[10]) != pid:
            continue
        tagged = re.search(r"kui\.window\.(\d+)\.frame\.(\d+)", fmt(row[6]))
        if tagged and int(tagged.group(1)) == main_window:
            # Command-buffer labels remain present when Immediate mode leaves
            # Metal's GPU frame-counter column empty. Never discard those rows.
            native = int(tagged.group(2))
            gpu[native].append((at, at + duration))
            for op, surface in accesses:
                if op == "Write":
                    app_writes.append((int(surface), (at + duration) / 1e9, native))

    starts = {int(row["frame"]): float(row["frame_started_ms"]) / 1000
              for row in csv_rows if row.get("frame_started_ms") and math.isfinite(float(row["frame_started_ms"]))}
    cpu_frames = {int(row["frame"]): row for row in csv_rows}

    surfaces = []
    scanouts = []
    owner = re.compile(r"\(" + re.escape(pid) + r"\):Frame ([\d,]+):Surface")
    for row in tables["displayed-surfaces-interval"]:
        surface = re.match(r"Surface (\d+)", fmt(row[9]))
        if surface:
            scanouts.append(dict(at=int(value(row[0])) / 1e9,
                                 duration_ms=int(value(row[1])) / 1e6,
                                 latency_ms=int(value(row[2])) / 1e6 if value(row[2]) else None,
                                 surface=int(surface.group(1)), display=fmt(row[3])))
        match = owner.search(fmt(row[9]))
        if match:
            surfaces.append(dict(at=int(value(row[0])) / 1e9,
                                 duration_ms=int(value(row[1])) / 1e6,
                                 latency_ms=int(value(row[2])) / 1e6 if value(row[2]) else None,
                                 frame=int(match.group(1).replace(",", "")),
                                 display=fmt(row[3])))

    client_surfaces = surfaces
    samples = sample_scanout_intervals(scanouts, vsyncs)
    resolved, ambiguous = resolve_scanout(samples, app_writes, list(compositor.values()))
    resource_attribution = bool(resolved)
    if resource_attribution:
        surfaces = resolved

    phases = {}
    for phase in ("scrolling", "typing"):
        times = [float(r["elapsed_ms"]) / 1000 for r in csv_rows if r["phase"] == phase]
        # Exclude the transition boundaries, including the CPU/GPU queue's
        # in-flight frames. Keep the sustained workload, not startup.
        offset = cpu["metadata"]["start_unix_seconds"] - start
        low, high = offset + min(times) + .2, offset + max(times) - .2
        direct = [s for s in client_surfaces if low <= s["at"] <= high]
        direct_coverage = sum(s["duration_ms"] for s in direct) / 1000 / (high - low)
        visible = [s for s in surfaces if low <= s["at"] <= high]
        durations = [s["duration_ms"] for s in visible]
        # A compositor can scan out fresh buffers containing the same app
        # frame. Count actual app updates, not the output-buffer rotations.
        version_key = "native_frame" if resource_attribution else "frame"
        shown = [s for i, s in enumerate(visible) if i == 0
                 or s[version_key] != visible[i - 1][version_key]]
        reported_hz = cpu["metadata"].get("display_reported_hz")
        cadence = presentation_cadence(shown, vsyncs, reported_hz)
        spans, busy = [], []
        for intervals in gpu.values():
            if low <= min(a for a, _ in intervals) / 1e9 <= high:
                spans.append((max(b for _, b in intervals) - min(a for a, _ in intervals)) / 1e6)
                busy.append(union_ms(intervals))
        coverage = sum(durations) / 1000 / (high - low)
        presentation_span = (shown[-1]["at"] - shown[0]["at"]) / (high - low) if shown else 0
        native_of = lambda s: s.get("native_frame") if resource_attribution else None
        frame_latency = [1000 * (s["at"] - offset - starts[native_of(s)])
                         for s in shown if native_of(s) in starts]
        gpu_to_display = [1000 * (s["at"] - max(end for _, end in gpu[native_of(s)]) / 1e9)
                          for s in shown if native_of(s) in gpu]
        work_sum = [float(cpu_frames[native]["cpu_work_ms"]) + union_ms(intervals)
                    for native, intervals in gpu.items()
                    if native in cpu_frames and cpu_frames[native]["phase"] == phase
                    and low <= min(at for at, _ in intervals) / 1e9 <= high]
        completion = [1000 * (max(end for _, end in intervals) / 1e9 - offset - starts[native])
                      for native, intervals in gpu.items()
                      if native in starts and native in cpu_frames and cpu_frames[native]["phase"] == phase
                      and low <= min(at for at, _ in intervals) / 1e9 <= high]
        surface_natives = {s.get("frame", s.get("native_frame")): native_of(s) for s in shown}
        for event in cadence["missed_presentations"]:
            previous = surface_natives.get(event["previous_surface_frame"])
            current = surface_natives.get(event["surface_frame"])
            event.update(previous_native_frame=previous, native_frame=current)
            if previous is None or current is None:
                continue
            nearby = []
            for native in range(max(1, previous - 1), current + 2):
                if native not in cpu_frames:
                    continue
                sample = cpu_frames[native]
                intervals = gpu.get(native, [])
                nearby.append(dict(
                    native_frame=native,
                    frame_start_seconds=offset + starts[native] if native in starts else None,
                    cpu_work_ms=float(sample["cpu_work_ms"]),
                    acquire_wait_ms=float(sample["acquire_wait_ms"]),
                    view_ms=float(sample["view_ms"]),
                    layout_ms=float(sample["layout_ms"]),
                    gpu_start_seconds=min(a for a, _ in intervals) / 1e9 if intervals else None,
                    gpu_end_seconds=max(b for _, b in intervals) / 1e9 if intervals else None,
                ))
            event["nearby_frames"] = nearby
        phases[phase] = dict(
            observed_surface_coverage=coverage,
            presentation_span_coverage=presentation_span,
            cadence_is_representative=.95 < coverage < 1.05,
            surface_attribution="GPU texture writes and compositor reads" if resource_attribution else "direct client labels; GPU/frame correlation unavailable",
            direct_client_label_observation=dict(
                measurement_scope="client-labelled surface changes; excludes content updates inside retained intervals",
                observed_surface_coverage=direct_coverage,
                cadence_is_representative=.95 < direct_coverage < 1.05,
                **presentation_cadence(direct, vsyncs, reported_hz)),
            compositor_ambiguous_source_groups=ambiguous,
            scanout_sampling="surface intervals subdivided at recorded hardware VSYNC pulses",
            repeated_app_scanouts=len(visible) - len(shown),
            source_frame_regressions=sum(native_of(b) < native_of(a) for a, b in zip(shown, shown[1:])
                                         if native_of(a) is not None and native_of(b) is not None),
            displayed_frames=len(shown),
            owned_surface_interval_ms=distribution(durations),
            **cadence,
            gpu_execution_span_ms=distribution(spans),
            gpu_busy_union_ms=distribution(busy),
            gpu_timing_scope="native-labelled command buffers; includes labelled blur passes, excludes unlabelled staging uploads",
            frame_start_to_display_ms=distribution(frame_latency),
            frame_correlation_coverage=len(frame_latency) / len(shown) if shown else 0,
            gpu_completion_to_display_ms=distribution(gpu_to_display),
            frame_start_to_gpu_completion_ms=distribution(completion),
            cpu_and_gpu_work_sum_ms=distribution(work_sum),
            cpu_and_gpu_work_over_budget=sum(value > BUDGET_MS for value in work_sum),
            metal_cpu_to_display_latency_ms=distribution([s["latency_ms"] for s in shown if s.get("latency_ms") is not None]),
            metal_latency_coverage=sum(s.get("latency_ms") is not None for s in shown) / len(shown) if shown else 0,
            window_trace_seconds=[low, high],
        )
    return {"target_pid": int(pid), "native_cpu": cpu, "physical_display": phases}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--single-case", action="store_true", help="ROOT is one capture folder")
    parser.add_argument("--max-latency-ms", type=float)
    parser.add_argument("--max-missed-refreshes", type=int)
    parser.add_argument("--max-frame-latency-ms", type=float, help="before_frame hook to physical display; requires tagged GPU commands")
    args = parser.parse_args()
    if args.single_case:
        report = metal_case(args.root)
        target = args.root / "summary.json"
        target.write_text(json.dumps(report, indent=2) + "\n")
        passed = True
        for phase, result in report["physical_display"].items():
            latency = result["metal_cpu_to_display_latency_ms"]
            representative = result["cadence_is_representative"]
            print(phase, "FPS", round(result["displayed_fps"] or 0, 3) if representative else "unavailable",
                  "latency p95 ms", round(latency["p95"], 3) if latency and representative else "unavailable",
                  "missed refreshes", result["missed_refresh_slots"] if representative else "unavailable",
                  "coverage", round(result["observed_surface_coverage"], 3))
            if not representative:
                print("  insufficient scanout coverage; raw JSON cadence is diagnostic")
                direct = result["direct_client_label_observation"]
                print("  direct client-label changes: FPS", round(direct["displayed_fps"] or 0, 3),
                      "missed refreshes", direct["missed_refresh_slots"],
                      "coverage", round(direct["observed_surface_coverage"], 3),
                      "(excludes retained-surface updates)")
            actual = result["frame_start_to_display_ms"]
            if actual:
                scope = "frame start to display" if representative else "observed subset frame start to display"
                print(" ", scope, "p95 ms", round(actual["p95"], 3),
                      "correlation coverage", round(result["frame_correlation_coverage"], 3))
            passed &= result["cadence_is_representative"]
            if args.max_latency_ms is not None:
                passed &= latency is not None and latency["p95"] <= args.max_latency_ms and result["metal_latency_coverage"] >= .95
            if args.max_missed_refreshes is not None:
                observed_hz = result["hardware_refresh_hz"]
                verified = (observed_hz is not None and abs(observed_hz - 120) < .5
                            and result["presentation_span_coverage"] >= .95)
                if not verified:
                    print("  refresh gate requires measured 120 Hz hardware timing across the sustained workload")
                passed &= verified
                passed &= result["missed_refresh_slots"] <= args.max_missed_refreshes
                passed &= result["source_frame_regressions"] == 0
            if args.max_frame_latency_ms is not None:
                passed &= actual is not None and actual["p50"] > 0 and actual["p95"] <= args.max_frame_latency_ms and result["frame_correlation_coverage"] >= .95
        print(target)
        raise SystemExit(0 if passed else 1)
    report = {"refresh_hz": 120, "frame_budget_ms": BUDGET_MS, "modes": {}}
    for mode in ("page", "window", "scroll"):
        result = {}
        folder = args.root / ("validated-" + mode)
        if folder.joinpath("metadata.json").exists():
            result["uninstrumented_native"], _ = cpu_case(folder)
        folder = args.root / ("gpu-" + mode)
        if any(folder.joinpath(name).exists() for name in ("metal-data.xml", "metal-data.xml.gz")):
            result["metal_trace"] = metal_case(folder)
        report["modes"][mode] = result
    foreground = args.root / "focused-window"
    if any(foreground.joinpath(name).exists() for name in ("metal-data.xml", "metal-data.xml.gz")):
        report["focused_window_validation"] = metal_case(foreground)
    target = args.root / "summary.json"
    target.write_text(json.dumps(report, indent=2) + "\n")
    print(target)


if __name__ == "__main__":
    main()
