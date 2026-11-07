#!/usr/bin/env python3
"""Exercise physical cadence, including holes in surface ownership."""
import runpy
import unittest
import csv
import datetime
import json
import tempfile
import xml.etree.ElementTree as ET
from pathlib import Path

analyzer = runpy.run_path(str(Path(__file__).with_name("analyze-native-perf.py")))
cadence = analyzer["presentation_cadence"]


class PhysicalCadence(unittest.TestCase):
    def test_sixty_hz_hardware_pulses_do_not_count_as_misses(self):
        times = [i / 60 for i in range(61)]
        shown = [dict(at=t, display="panel") for t in times]
        result = cadence(shown, {"panel": times})
        self.assertAlmostEqual(result["hardware_refresh_hz"], 60)
        self.assertEqual(result["missed_refresh_slots"], 0)

    def test_slower_presentations_accumulate_missed_slots(self):
        for fps in (90, 100):
            with self.subTest(fps=fps):
                result = cadence([dict(at=i / fps) for i in range(fps + 1)])
                self.assertEqual(result["missed_refresh_slots"], 120 - fps)

    def test_every_refresh(self):
        result = cadence([dict(at=n / 120) for n in range(31)])
        self.assertAlmostEqual(result["displayed_fps"], 120)
        self.assertEqual(result["missed_refresh_slots"], 0)

    def test_unowned_gap_is_a_missed_refresh(self):
        # Every visible interval is short: counting only interval lengths
        # would falsely report 120 Hz. Actual presentations skip a slot.
        shown = [dict(at=n / 120, duration_ms=1000 / 120) for n in [0, 1, 3, 4]]
        result = cadence(shown)
        self.assertAlmostEqual(result["displayed_fps"], 90)
        self.assertEqual(result["missed_refresh_slots"], 1)
        self.assertAlmostEqual(result["missed_refresh_percent"], 25)
        self.assertAlmostEqual(result["displayed_frame_interval_ms"]["maximum"], 1000 / 60)

    def test_export_order_and_small_timestamp_jitter(self):
        result = cadence([dict(at=t) for t in [.025, 0, .008336, .016665]])
        self.assertAlmostEqual(result["displayed_fps"], 120)
        self.assertEqual(result["missed_refresh_slots"], 0)

    def test_one_presentation_cannot_establish_cadence(self):
        self.assertIsNone(cadence([dict(at=1)])["displayed_fps"])

    def test_missed_refresh_retains_the_two_presentations_for_drilling_down(self):
        result = cadence([dict(at=n / 120, frame=n + 100) for n in [4, 0, 1, 3]])
        self.assertEqual(len(result["missed_presentations"]), 1)
        event = result["missed_presentations"][0]
        self.assertEqual(event["previous_surface_frame"], 101)
        self.assertEqual(event["surface_frame"], 103)
        self.assertEqual(event["missed_slots"], 1)
        self.assertAlmostEqual(event["interval_ms"], 1000 / 60)

    def test_hardware_pulses_distinguish_app_miss_from_display_pause(self):
        shown = [dict(at=n / 120, display="built-in") for n in [0, 2, 4]]
        result = cadence(shown, {"built-in": [0, 1 / 120, 2 / 120, 4 / 120],
                                 "external": [3 / 120]})
        events = result["missed_presentations"]
        self.assertEqual(events[0]["intervening_hardware_vsync_seconds"], [1 / 120])
        self.assertEqual(events[1]["intervening_hardware_vsync_seconds"], [])

    def test_absent_hardware_trace_does_not_imply_absent_refresh(self):
        event = cadence([dict(at=0), dict(at=2 / 120)])["missed_presentations"][0]
        self.assertIsNone(event["intervening_hardware_vsync_seconds"])


class CompositedFrames(unittest.TestCase):
    def test_retained_scanout_surface_can_contain_a_new_frame_each_refresh(self):
        expand = analyzer["sample_scanout_intervals"]
        pulses = [n / 120 for n in range(4)]
        samples = expand([dict(at=0, duration_ms=25, surface=9, display="built-in")],
                         {"built-in": pulses})
        self.assertEqual([s["at"] for s in samples], pulses[:3])
        self.assertAlmostEqual(sum(s["duration_ms"] for s in samples), 25)
        writes = [(1, -.005, 10), (1, .005, 11), (1, .013, 12)]
        groups = [dict(reads={1: -.003}, writes={9: -.002}),
                  dict(reads={1: .006}, writes={9: .007}),
                  dict(reads={1: .014}, writes={9: .015})]
        result, _ = analyzer["resolve_scanout"](samples, writes, groups)
        self.assertEqual([s["native_frame"] for s in result], [10, 11, 12])
        self.assertAlmostEqual(cadence(result)["displayed_fps"], 120)

    def test_normal_scanout_boundaries_do_not_duplicate_hardware_pulses(self):
        expand = analyzer["sample_scanout_intervals"]
        shown = [dict(at=n / 120 + .0000004, duration_ms=1000 / 120, display="built-in")
                 for n in range(3)]
        self.assertEqual(expand(shown, {"built-in": [n / 120 for n in range(4)]}), shown)
        self.assertEqual(expand(shown, {}), shown)

    def test_scanout_uses_the_version_sampled_by_the_compositor(self):
        resolve = analyzer["resolve_scanout"]
        writes = [(1, .001, 10), (1, .007, 11)]
        compositor = [dict(reads={1: .003}, writes={9: .005})]
        shown = [dict(at=.008, surface=9)]
        result, ambiguous = resolve(shown, writes, compositor)
        self.assertEqual(ambiguous, 0)
        self.assertEqual(result[0]["native_frame"], 10)

    def test_changing_compositor_buffers_does_not_invent_app_updates(self):
        resolve = analyzer["resolve_scanout"]
        writes = [(1, .001, 10)]
        compositor = [dict(reads={1: .002}, writes={8: .003}),
                      dict(reads={1: .010}, writes={9: .011})]
        result, _ = resolve([dict(at=.008, surface=8), dict(at=.016, surface=9)], writes, compositor)
        self.assertEqual([e["native_frame"] for e in result], [10, 10])

    def test_ambiguous_versions_are_unavailable(self):
        resolve = analyzer["resolve_scanout"]
        result, ambiguous = resolve([dict(at=.008, surface=9)],
            [(1, .001, 10), (2, .001, 11)],
            [dict(reads={1: .002, 2: .002}, writes={9: .003})])
        self.assertEqual(ambiguous, 1)
        self.assertEqual(result, [])


class NativeCommandBuffers(unittest.TestCase):
    def test_missing_gpu_frame_counter_and_shared_compositor_frame(self):
        # Immediate presentation can leave Metal's frame-counter column empty.
        # Two compositor command buffers in one GPU frame may sample different
        # versions of the same app texture. Neither is an unknown app frame.
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            date = "2026-11-06T00:00:00+00:00"
            offset = datetime.datetime.fromisoformat(date).timestamp()
            (folder / "metadata.json").write_text(json.dumps(dict(
                start_unix_seconds=offset, duration_per_phase_seconds=.5, native_window_id=0)))
            fields = ["frame", "phase", "elapsed_ms", "frame_started_ms", "cpu_work_ms",
                      "cpu_total_ms", "render_submit_ms", "acquire_wait_ms", "view_ms", "layout_ms"]
            with (folder / "frames.csv").open("w") as stream:
                writer = csv.DictWriter(stream, fieldnames=fields); writer.writeheader()
                for base, phase, first in [(1, "scrolling", 1), (2, "typing", 5)]:
                    for i, at in enumerate([base, base + .21, base + .218333333, base + .5]):
                        row = dict.fromkeys(fields, "0")
                        row.update(frame=first + i, phase=phase, elapsed_ms=at * 1000,
                                   frame_started_ms=at * 1000, cpu_work_ms=1, cpu_total_ms=1)
                        writer.writerow(row)
            (folder / "metal-toc.xml").write_text(
                '<trace-toc><run><info><target><process pid="99"/></target>'
                f'<summary><start-date>{date}</start-date></summary></info><data>'
                '<table schema="metal-gpu-intervals"/><table schema="displayed-surfaces-interval"/>'
                '</data></run></trace-toc>')
            root = ET.Element("trace-query-result")
            gpu = ET.SubElement(root, "node", xpath="/trace-toc/run/data/table[1]")
            shown = ET.SubElement(root, "node", xpath="/trace-toc/run/data/table[2]")
            def row(node, count):
                result = ET.SubElement(node, "row")
                return [ET.SubElement(result, "cell") for _ in range(count)]
            for base, first in [(1, 1), (2, 5)]:
                for i in range(2):
                    native = first + i + 1
                    at = base + .212 + i / 120
                    app = row(gpu, 18)
                    app[0].text = str(round(at * 1e9)); app[1].text = "1000000"
                    app[6].set("fmt", f"kui.window.0.frame.{native}:root")
                    ET.SubElement(app[10], "pid").text = "99"
                    app[13].set("fmt", "Write Surface: 1(2200×1480:ARGB)")
                    ws = row(gpu, 18)
                    ws[0].text = str(round((at + .002) * 1e9)); ws[1].text = "1000000"
                    ws[3].text = str(base + 40)
                    ws[10].set("fmt", "WindowServer (10)")
                    ws[13].set("fmt", f"Read Surface: 1(2200×1480:ARGB) Write Surface: {9+i}(3456×2234:ARGB)")
                    ws[15].text = str(base * 100 + i)
                    event = row(shown, 10)
                    event[0].text = str(round((base + .22 + i / 120) * 1e9))
                    event[1].text = "8333333"; event[2].text = "10000000"
                    event[3].set("fmt", "Built-In Display 1")
                    event[9].set("fmt", f"Surface {9+i}")
            ET.ElementTree(root).write(folder / "metal-data.xml", encoding="unicode")
            result = analyzer["metal_case"](folder)
            for phase in ("scrolling", "typing"):
                actual = result["physical_display"][phase]
                self.assertEqual(actual["displayed_frames"], 2)
                self.assertEqual(actual["frame_correlation_coverage"], 1)
                self.assertEqual(actual["gpu_busy_union_ms"]["count"], 2)


if __name__ == "__main__":
    unittest.main()
