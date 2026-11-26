# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0

import unittest

import reference


class ReferenceObservationTests(unittest.TestCase):
    def test_manifest_rotations_use_prusa_axis_flags(self):
        self.assertEqual(reference.rotation_args({}), [])
        self.assertEqual(reference.rotation_args({"y": 90, "x": 90, "z": -90}),
                         ["--rotate-x", "90", "--rotate-y", "90", "--rotate", "-90"])

    def test_absent_mesh_evidence_is_unknown_not_closed(self):
        self.assertIsNone(reference.mesh_info("")["open_edges"])
        self.assertIsNone(reference.mesh_info("")["manifold"])
        self.assertFalse(reference.mesh_info("")["positive_geometry"])
        info = reference.mesh_info("manifold = yes\nsize_x = 8\nsize_y = 0.4\nsize_z = 0.8\nvolume = 2.56\n")
        self.assertTrue(info["positive_geometry"])
        self.assertTrue(info["manifold"])
        self.assertIsNone(info["open_edges"])
        self.assertFalse(reference.mesh_info("manifold = no\n")["manifold"])

    def test_nonempty_slice_needs_model_extrusion_not_travel_skirt_or_prime(self):
        lines = ["M83", ";Z:0.2", ";TYPE:Skirt/Brim", "G1 X1 E1", ";TYPE:External perimeter",
                 "G1 X2", "G1 E1", "G1 X3 E-1", "; filament used [mm] = 3"]
        self.assertFalse(reference.program_info(lines, 0.2)["nonempty_model"])
        lines.append("G1 X4 E1")
        self.assertTrue(reference.program_info(lines, 0.2)["nonempty_model"])

    def test_absolute_extrusion_reset_and_missing_filament_summary(self):
        lines = ["M82", ";Z:0.2", ";TYPE:Perimeter", "G92 E5", "G1 X1 E4", "G92 E0", "G1 X2 E1"]
        report = reference.program_info(lines, 0.2)
        self.assertEqual(report["model_extrusion_moves"], 1)
        self.assertFalse(report["nonempty_model"])
        lines.append("; filament used [mm] = 1")
        self.assertTrue(reference.program_info(lines, 0.2)["nonempty_model"])


if __name__ == "__main__":
    unittest.main()
