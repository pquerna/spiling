<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Closed-profile SQLite golden

`profile-v1.spil` is synthetic container conformance data, not a Core project: its
root/guard deliberately have no project-domain JSON meaning. The upstream SQLite
oracle in `checked_in_sqlite_golden_is_inspected_without_the_creator` inspects it
without executing the creator, then the container checker independently admits it.
It has one current root, one RAW member and no previous snapshot.

Regenerate deliberately after reviewing an intended physical representation change:

```sh
SPILING_REGENERATE_PTAC_GOLDEN=1 cargo test -p spiling-core-ptac checked_in_sqlite_golden
```

The exact root is `PTAC golden root\n`, member is `independent exact asset bytes\n`
with role 1, and guard is `PTAC golden guard\n` (each `\n` is a single LF byte).
The fixture is original synthetic Spiling material under the repository license.
