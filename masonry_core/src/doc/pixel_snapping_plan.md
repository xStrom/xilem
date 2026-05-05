# Pixel snapping and DPI scale handling plan

This document describes a target design for pixel snapping, DPI scale handling, and
visual geometry in Masonry.

The core distinction is between **ideal geometry** and **visual geometry**.
Ideal geometry is fractional, logical, and stable across DPI changes. It is used
for layout, transforms, text metrics, and semantic relationships such as baseline
alignment. Visual geometry is derived after compose, can be snapped to the
physical pixel grid, and is used for visual boxes, hit testing, clips, bounds,
accessibility geometry, and paint box APIs.

## Goals

The system should uphold the following goals and invariants:

- Widget-local coordinates are logical layout coordinates, not device pixels.
- Public widget APIs and internal widget methods use the same logical coordinate
  model unless a method explicitly says otherwise.
- Device pixel conversion happens inside Masonry at visual/render/platform
  boundaries.
- Layout produces ideal fractional geometry and should not depend on DPI scale for
  normal widget sizing.
- DPI changes should generally not cause layout changes or widget tree logic
  changes.
- Pixel snapping uses the full window transform and DPI scale, not only
  the parent-local layout origin.
- Visual box resolution happens after compose, once the widget's full transform
  to the window is known.
- Visual box geometry is available before hit testing, accessibility coordinate
  updates, IME coordinate updates, and paint.
- Hit testing and paint should use the same visual box geometry.
- Widget outer border-box edges should be pixel aligned when snapping is enabled
  and the full transform allows it.
- Sibling widget borders should remain gapless after snapping. For example, three
  equal children in a 100 device-pixel container may resolve to widths like 33,
  33, and 34 device pixels.
- Snapping should be deterministic and should avoid accumulating local rounding
  errors through the tree.
- Snapping is disabled for widgets whose full transform is not axis-aligned,
  such as transforms with rotation or shear.
- Snapping can be disabled for widgets that opt into subpixel motion or transform
  animation.
- Ideal geometry remains available even when visual geometry is snapped.
- Box edge snapping and text baseline snapping are separate constraints.
- Baseline alignment during layout is based on ideal baseline anchors.
- Text baseline snapping uses ideal baseline coordinates, not visual box edges.
- Multi-line text can snap each line baseline independently at paint time.
- Widget paint methods continue to work in widget-local logical coordinates.
- Paint box APIs should expose visual geometry where that is useful for
  crisp edge-to-edge drawing.
- Paint code that needs layout semantics, text metrics, or ideal placement can
  access ideal geometry explicitly.
- Hairline strokes, separators, focus rings, grid lines, and similar precise
  shapes should use snap-aware paint helpers rather than ad hoc DPI math.
- Fractional DPI scale factors, such as 1.25 and 1.5, are first-class cases.
- Fractional design values, such as a 1.5 logical-pixel border, remain valid and
  are not forcibly quantized by default.
- DPI-aware image resources should be selectable by window scale, while preserving
  stable logical layout size.
- Image painting should be able to use sharp high-DPI resources without upscaling
  low-DPI logical-sized images.
- The per-widget storage cost should remain small. Store compact visual
  data and derive specialized snap values lazily when possible.

## Part 1: Restore Logical Widget Coordinates

Widget-local and Masonry-internal geometry should return to logical layout
coordinates instead of the partially implemented device-pixel model.

- Audit documentation and APIs that currently describe widget-local values as
  device pixels.
- Update layout, measurement, property resolution, and widget context docs to
  describe logical layout coordinates.
- Remove hardcoded `scale = 1.0` workarounds as part of replacing the incomplete
  device-pixel conversion model.
- Keep platform input and output conversions at explicit boundaries.
- Normalize pointer positions into window logical coordinates before hit testing
  and widget event handling.
- Ensure `Length`, `Dim`, `LayoutSize`, `LenReq`, and related layout types have
  clear logical-vs-device semantics.
- Preserve the ability for specialized APIs to request physical-device-pixel
  behavior explicitly.

The failed assumption in the current partial model is that widget-local pixels can
be device pixels. Once arbitrary transforms are supported, a local unit can map to
fractional, scaled, or otherwise transformed device coordinates. It is therefore
more accurate to treat widget-local coordinates as logical layout coordinates and
perform physical pixel conversion only when resolving visual geometry.

This also keeps layout stable across monitor DPI changes. A widget that asks for a
100px logical size should not change its layout size just because the window moved
from a 1.0x display to a 1.5x display. The visual representation may become
sharper or use more physical pixels, but the ideal layout remains unchanged.

## Part 2: Resolve Visual Boxes After Compose

Visual box resolution should move out of layout and happen after compose has
produced each widget's full transform to the window. The first implementation
resolves visual boxes by pixel-snapping eligible layout boxes.

- Stop snapping child layout geometry in `place_child`.
- Store layout origins and sizes as ideal fractional logical geometry.
- Compute the ideal `window_transform` during compose as today, but without
  relying on pre-snapped layout points.
- Add a post-compose visual step, or an explicit phase at the end of compose,
  that resolves visual box geometry.
- Convert ideal local box edges into device coordinates with the full
  local-to-window transform and window-to-device scale.
- Snap the outer border-box edges in device space when the full transform is
  eligible.
- Map the snapped device border-box back into widget-local logical coordinates as
  visual geometry.
- Use the same visual boxes for paint box APIs, hit testing, clips,
  bounds, accessibility geometry, and IME fallback geometry.
- Disable box snapping for non-axis-aligned full transforms.
- Add a future opt-out for subpixel motion or transform animation.
- Keep ideal geometry available alongside visual geometry.
- Keep stored visual data compact, preferably one primary visual border-box
  rect plus existing or derived bounding data.

The visual box is not a replacement for ideal layout geometry. It is the
box that paint, hit testing, and platform geometry need to agree on. The ideal
geometry remains the source of truth for layout, transform composition, text
metrics, and baseline relationships.

A minimal storage shape should be enough for a first implementation. For example,
`WidgetState` can keep ideal fields such as `origin`, `border_box_size`,
`border_box_insets`, `paint_insets`, and `window_transform`, and add compact
visual data such as a `visual_border_box` rect plus a window-space
bounding box. Padding-box, content-box, paint-box, and specialized snap values
can usually be derived lazily from the visual border-box, ideal insets, and the
snap context.

The visual local rect is not obtained by reversing the rounding operation.
Rounding in device space is lossy. Instead, Masonry chooses representative local
logical coordinates that map to the snapped device edges under the ideal
local-to-device transform. These coordinates are visual geometry, not
recovered ideal layout geometry.

## Part 3: Snap Multi-Line Text Baselines From Ideal Geometry

Text baseline snapping should use ideal baseline anchors, so visual baseline
alignment survives visual box snapping.

- Keep widget baseline metrics fractional and logical.
- Keep parent baseline alignment, such as Flex first-baseline alignment, as an
  ideal layout relationship.
- Do not derive snapped text baselines from the visual box top edge.
- Make text layout libraries such as Parley produce ideal fractional metrics.
- Avoid asking text layout to quantize line positions before the final transform
  to the window is known.
- Provide a `SnapContext` that can map ideal local baseline coordinates into
  device space, round them, and return visual coordinates for paint.
- Snap each text line baseline independently during paint, caret positioning,
  selection painting, IME geometry, and text hit testing when those systems need
  visual text positions.
- Do not store every snapped line baseline in `WidgetState`.
- Initially rely on mathematically equal ideal baselines snapping to the same
  device coordinate for compatible transforms.
- Consider explicit baseline snap groups later if floating-point edge cases near
  half-pixel boundaries become observable.

Baseline alignment and visual box snapping are separate constraints. During layout, a
parent such as Flex can continue to place children so that:

```text
child_origin_y + child_first_baseline_y = common_baseline_y
```

After compose, if the children have compatible axis-aligned transforms, each
child's ideal first baseline maps to the same device coordinate. Snapping those
ideal baselines independently therefore produces the same snapped visual baseline.
The crucial rule is that baseline snapping ignores any visual shift caused
by box snapping.

For multi-line text, only the widget's first and last baselines participate in
external layout relationships, but every line has a visual baseline. Those line
baselines should remain ideal fractional values until paint or another visual text
operation runs. The snap context can then resolve each
line baseline lazily without increasing per-widget state.

## Part 4: Add Snap-Aware Paint Helpers

Precise pixel-aligned drawing inside widgets should be expressed through
intent-specific paint helpers built on the same snap context.

- Expose a `SnapContext` or equivalent through `PaintCtx`.
- Let widgets continue painting in local logical coordinates.
- Provide helpers for filled rectangles whose edges snap to device pixel edges.
- Provide helpers for horizontal and vertical hairlines with exact physical-pixel
  thickness.
- Provide helpers for device-pixel stroke widths where an approximate scalar is
  useful.
- Make direction-aware helpers available for non-uniform scale, such as separate
  horizontal and vertical device-pixel widths.
- Provide baseline helpers for text painting.
- Provide image destination or sampling helpers where pixel-aligned image drawing
  matters.
- Avoid generic path point-rounding as a default behavior.
- Treat complex filled and stroked paths as antialiased ideal geometry unless the
  widget uses a more specific snap-aware helper.
- Keep helper behavior explicit about when it is exact and when it is an
  approximation.

Different drawing operations need different snapping rules. A filled rectangle
wants its edges snapped to integer device coordinates. A 1-device-pixel hairline
usually wants its centerline snapped to the correct device-pixel center. A text
baseline wants the baseline anchor snapped, not the text bounding box. An image may
need its destination rect, sampling origin, or selected resource variant adjusted
depending on the intended behavior.

For this reason, the snap API should be intent-based rather than only exposing raw
point rounding. General helpers such as "local stroke width for N device pixels"
are still useful, especially for diagonal lines and approximate strokes, but they
should be documented as exact only for compatible uniform-scale transforms. More
specific helpers should be preferred for common UI primitives such as separators,
focus rings, grid lines, borders, and text baselines.

Complex `BezPath` snapping should be conservative. Rounding every point or control
point can distort curves, alter joins, and change fill behavior. For arbitrary
paths, preserving ideal geometry with antialiasing is usually better. Snap-aware
path behavior should be introduced only for clear cases, such as axis-aligned
polylines, rectangles, rounded rectangles, and icon-like shapes with a snapped
anchor.
