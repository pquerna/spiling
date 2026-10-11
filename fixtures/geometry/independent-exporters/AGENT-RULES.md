<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Independent original STEP exporter rules

Own original OCCT-exported box/cylinder/through-hole diagnostic inputs, generation recipe, frozen hashes and notices. `generate.py` uses tooling-only `cadquery-ocp==7.9.3.1`; it is not a Spiling runtime dependency or alternate geometry backend. Require BRepCheck validity before export, fixed header timestamp, AP214 and original dimensions. Never rewrite frozen source bytes after generation; use a new fixture version for source changes. Source and recipe are original OSL-3.0 work; do not copy third-party CAD or relabel exporter licensing.

Regenerate/check with `UV_CACHE_DIR=PATH uv run --with cadquery-ocp==7.9.3.1 python fixtures/geometry/independent-exporters/generate.py [--check]`. Exercise these sources through the actual public native facade and CLI. Record accepted/unsupported/error outcomes in quality evidence without widening admission or silently repairing input. Exporter bindings are Apache-2.0 and OCCT LGPL-2.1 with exception; these tools are not bundled with Spiling.
