<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Imported Google schemas rules

- Keep imported .proto files and LICENSE byte-identical to the commit recorded in README.md. Preserve upstream attribution; never apply project headers or formatting to these files.
- Update provenance and validate generation when changing the pinned upstream source. The schemas are not a dependency on hosted Google services.
