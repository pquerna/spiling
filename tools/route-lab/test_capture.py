# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Keep native routing captures tied to emitted order and physical strokes."""
import json
from pathlib import Path
import tempfile
import unittest

from capture import NATIVE_LOCKED, fixture, native


class NativeCaptureTests(unittest.TestCase):
    def capture(self, root, start=1.):
        export = root / 'export'
        export.mkdir()
        path = dict(points_mm=[[1., 2., .4], [4., 2., .4], [4., 5., .4]],
                    width=dict(kind='per_edge', widths_mm=[.45, .6]),
                    physical_z_mm=[.2, .4], kind='perimeter')
        (export / 'plan.json').write_text(json.dumps(dict(layers=[dict(z_mm=.4, paths=[path])])))
        (export / 'program.gcode').write_text(
            f'M83\nG1 Z0.4\nG1 X{start} Y2\nG1 X4 Y2 E0.2\nG1 X4 Y5 E0.3\n')

    def test_native_capture_preserves_widths_interval_direction_and_order(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.capture(root)
            items = native(root, [0])
            self.assertEqual(items[0]['widths'], [.45, .6])
            self.assertEqual(items[0]['height'], .2)
            self.assertEqual(items[0]['z'], .4)
            text = fixture('test native layer', items, NATIVE_LOCKED)
            self.assertIn('S 0 0.200 0.400 1 perimeter', text)
            self.assertIn('P 4.000 2.000 0.450000', text)
            self.assertIn('P 4.000 5.000 0.600000', text)

    def test_native_capture_refuses_plan_program_order_mismatch(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.capture(root, start=0.)
            with self.assertRaisesRegex(ValueError, 'plan order differs'):
                native(root, [0])

    def test_stacked_capture_retains_declared_lower_band_dependencies(self):
        lower = dict(z=.2, height=.2, role='fill', widths=[.45], points=[(0., 0.), (2., 0.)])
        upper = dict(z=.4, height=.2, role='bridge', widths=[.6], points=[(0., 0.), (2., 0.)])
        text = fixture('stack', [lower, upper], NATIVE_LOCKED)
        self.assertIn('S 0 0.000 0.200 0 fill', text)
        self.assertIn('S 1 0.200 0.400 1 bridge', text)
        self.assertIn('R 1 0', text)


if __name__ == '__main__':
    unittest.main()
