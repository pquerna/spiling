<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Licensing Spiling

## Canonical license and scope

Original Spiling code and documentation are licensed under the **Open Software License version 3.0**, SPDX identifier **`OSL-3.0`**. [Root LICENSE.md](../LICENSE.md) is the canonical repository copy. Its fenced text is the unmodified license, not a summary or a project-specific license variant.

Official references:

- [SPDX OSL-3.0](https://spdx.org/licenses/OSL-3.0.html).
- [Open Source Initiative OSL-3.0](https://opensource.org/license/OSL-3.0).
- [SPDX machine-readable source used for LICENSE.md](https://github.com/spdx/license-list-data/blob/main/json/details/OSL-3.0.json), field `licenseText`.

Keep the license's own Lawrence Rosen copyright notice intact. Do not insert Spiling ownership notices or extra conditions into the license text. The README, file notices, and package metadata apply the license to Spiling; they do not amend it. The full license governs if this operational guide is incomplete.

Third-party dependencies, vendored code, fonts, fixtures, and other imported assets retain their own licensing and attribution. This project declaration does not relicense them. Check provenance and compatibility before incorporating or distributing them; a dependency's SPDX label alone does not establish compatibility.

## Standard file header

Use this three-line notice in the file's native comment syntax:

```text
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
```

The explicit licensing sentence follows the notice specified in the license's opening paragraph. Keep it adjacent to the copyright notice; SPDX provides machine-readable identification. A full license copy in every source file is unnecessary.

`Spiling contributors` collectively identifies the authors of original project material; it is not a legal entity or a copyright assignment. Use the actual individual or organization when ownership is known or a contributor requires attribution. Add separate `SPDX-FileCopyrightText` lines for additional holders. Preserve existing ownership notices rather than replacing them with the collective label.

Use the year of first authorship; extend to a range for later substantive changes where appropriate. Do not mechanically refresh every year or add a modifier's copyright for an insignificant edit. The year in these examples is not a fixed value for future files.

### Rust

Use ordinary line comments before module docs, imports, or items; do not turn legal notices into rustdoc:

```rust
// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Module documentation follows the legal header.
```

### TypeScript, TSX, JavaScript, JSX, CSS, and SCSS

Use a single retained-notice block, not JSDoc:

```typescript
/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
```

For scripts with a shebang, put the header immediately after the shebang. Configure bundling/minification to preserve legal comments or extract them into shipped notices; the comment marker alone does not prove retention. Verify the packaged output.

### Markdown, HTML, and SVG

Place an HTML comment before the main content:

```html
<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->
```

Preserve required document prologues: an XML declaration or HTML doctype precedes the comment. Root `LICENSE.md` is deliberately exempt from the project header so the canonical license text and its own copyright remain unambiguous.

### TOML, YAML, shell, and Python

Use `#` comments, after any required shebang or encoding declaration:

```toml
# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
```

### Commentless, binary, generated, and imported files

- Do not insert comments into JSON or binary formats. For project-authored files needing individual attribution, use an adjacent `<filename>.license` sidecar containing the standard notice; include it in distributions.
- For generated Rust/TypeScript, make the generator emit the header when the output is project-owned. Do not hand-edit generated output. Third-party generator templates or embedded code may require their own notices; check those terms.
- Do not hand-edit lockfiles or tool-managed output to add headers. Preserve tool-produced notices and describe any necessary licensing exceptions in the owning area when it is introduced.
- Preserve imported files' original license/copyright notices and required attribution. Record provenance and any modifications. Do not stamp them `OSL-3.0` merely because they are in this repository.

## Cargo and JavaScript package metadata

When the Cargo workspace exists, declare:

```toml
[workspace.package]
license = "OSL-3.0"
```

Each project-owned crate, including executable crates, inherits it in its existing package table:

```toml
[package]
license.workspace = true
```

Use Cargo's SPDX `license` field, not `license-file` as a substitute for the recognized identifier. Ensure each published crate includes the canonical license text and required notices; workspace-root files are not automatically included in every crate archive. If packaging needs a local copy, produce it from root `LICENSE.md` and check for drift rather than maintaining a second canonical version.

Each project-owned npm/pnpm package declares:

```json
{
  "license": "OSL-3.0"
}
```

Include the canonical license and required notices in published packages and desktop/CLI distributions. Verify archive contents, not just source-tree metadata. Package privacy or an SPDX declaration does not remove applicable license obligations.

## Applying the license and shipping changes

Before committing new original material:

1. Confirm authorship and the right to license the contribution. Do not add material of unknown provenance.
2. Add the appropriate short header or sidecar and preserve existing notices.
3. Keep README licensing, package metadata, and the root license consistent.
4. Update affected canonical area rules if licensing constraints or distribution behavior change.

Before publishing or externally deploying:

- Retain copyright, patent, trademark, licensing, and designated Attribution Notices. Section 6 also requires a prominent Attribution Notice informing recipients when the Original Work has been modified; use a shipped, recipient-visible modification notice, not merely hidden Git history. Keep such release history out of canonical agent rules/vision.
- Distribute or communicate covered Original Work and Derivative Works under OSL-3.0 as required by section 1(c). Do not infer that every independent dependency or collective-work component is thereby relicensed.
- Meet section 3's source-code availability requirement, including available documentation describing modification. If using a source repository instead of accompanying source, provide inexpensive, convenient access for as long as distribution continues. Check that the supplied source corresponds to the distributed work.
- Treat **External Deployment as distribution** under section 5. Its definition includes use by persons other than “You,” not only downloadable binaries or public network services; section 14 defines “You” for individuals and legal entities. A private-hosting label is not by itself an exemption.
- Make a reasonable effort under the circumstances to obtain recipients' express assent to the license under section 9. Merely displaying an SPDX identifier does not establish that this requirement has been met. Choose and document a suitable assent mechanism before distribution.
- Review native dependencies, packaging, source access, notices, and the chosen assent mechanism before release. Preserve the license's actual warranty, liability, patent, and termination terms; do not substitute a generic permissive-license disclaimer.

This is an engineering application guide, not legal advice or a guarantee of compliance. Resolve uncertain ownership, derivative-work boundaries, license compatibility, and distribution/assent questions through qualified legal review rather than inventing exceptions.
