<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Release source availability

Spiling source is maintained at https://github.com/pquerna/spiling under OSL-3.0. A distribution must identify its source commit and supply the corresponding preferred form for modification, including documentation, manifests, lockfiles, generated contracts and build scripts.

`pnpm package` records the current commit, worktree modification status, toolchains and resolved dependencies in the bundled `notices/provenance.json`. A dirty development package is not a releasable artifact: its source cannot be recovered from the recorded commit alone. Before a public release, commit all included source and publish that commit, or provide an accompanying complete source archive that includes modifications.

Recipients can retrieve a clean release's source with:

```sh
git clone https://github.com/pquerna/spiling.git
cd spiling
git checkout <commit-recorded-in-notices/provenance.json>
pnpm bootstrap
```

The repository must remain inexpensively and conveniently accessible while that release is distributed, as required by OSL section 3. A release owner must verify recipient access; a private URL or missing commit does not meet this procedure. Release source excludes private research and third-party customer CAD.

The desktop displays the complete license offline and requires explicit assent before entering the workbench. The acknowledgement is local and versioned; it is not telemetry or a copyright assignment. CLI diagnostics are developer tools; a public CLI distribution must be accompanied by a documented recipient-assent procedure suited to its distribution channel. No B0 development smoke establishes legal compliance for every deployment.

Before release, confirm source availability, notices, dependency licensing, the recipient-assent mechanism, platform signing/install behavior and the hardware support matrix. Record a prominent modification attribution when distributing a derivative of an existing Original Work. Seek qualified review where obligations or compatibility are uncertain.
