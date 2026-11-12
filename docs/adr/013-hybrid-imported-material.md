<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# ADR013: retained analytic and faceted imported material

## Decision

Each definition has one adopted material interpretation. STEP retains its imported SolidCore BREP. STL and 3MF retain a normalized indexed surface complex with grouped source regions, shared edges, admitted analytic patches and faceted residuals. This amends the BREP-only source wording in ADR005 and ADR012; it does not introduce a second slicer or make the display mesh authoritative.

The format decoder isolates ZIP/XML and STL syntax. Geometry owns unit and affine normalization, winding, region recovery, approximation bounds and shared section preparation. The same section, wall, skin, infill, support, routing, realization and independent emitted-program checks consume the prepared material. Operations needing a native-only capability refuse explicitly until their common geometry obligations are implemented.

Original bytes remain immutable provenance. Normalized geometry is a separate hashed immutable PTAC object. Definition identity binds both original and adopted geometry plus its interpretation selector. Saved return restores this exact interpretation instead of rerunning recognition. Region references distinguish mesh regions from STEP entities. Triangle count is not face count.

Exact-coordinate welding changes no material. Winding correction preserves nested void shells. Shape-changing repairs require a retained explicit policy. Planar recovery groups connected coplanar facets. Curved recovery bounds triangle-interior deviation and requires compatible shared trims; failed proposals remain faceted. Recovered analytics do not claim original CAD identity or native exact certification.

3MF references use package-part URI plus object ID. Components, units, affine transforms and repeated instances resolve before source publication. Required material-changing extensions and ambiguous roles refuse rather than fabricate geometry. Foreign process/printer presets never replace the active project setup. Negative parts must subtract within their logical object or refuse; modifiers and support controls are never positive model material.

## Alternatives and consequences

A mesh-only slicer would duplicate planning and verification. Turning every triangle into a synthetic BREP would multiply topology and obscure source identity. Requiring complete analytic reconstruction would exclude ordinary printable meshes. A mixed surface complex supports incremental recovery while retaining one material interpretation.

Native certificates remain capability-specific. Import, rendering, nominal replay and geometric recovery do not establish calibrated strength, physical supports, swept-nozzle clearance or machine readiness. Acceptance records must name actual complete journeys and remaining refusals.
