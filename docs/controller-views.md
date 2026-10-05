# Drawing a controller's views

The editor shows each controller as one or more line drawings, called views: the
front, the back, and so on. A view is an SVG file. The same file holds the button
shapes the user clicks, so the drawing is the only copy of the geometry.

The worked example is the 8BitDo Pro 3 in
[`crates/controller-core/controllers/pro3/`](../crates/controller-core/controllers/pro3/).
The loader is [`crates/controller-core/src/view.rs`](../crates/controller-core/src/view.rs).
The format is [`schemas/controller-description-v1.schema.json`](../schemas/controller-description-v1.schema.json).

## 1. Create the model folder

Make `crates/controller-core/controllers/<model>/`. Put `description.json` and one
SVG per view in it:

```text
crates/controller-core/controllers/pro3/
├── description.json
├── front.svg
└── back.svg
```

## 2. Draw the art within the format

The art is original line art in two colours. The editor swaps both colours to match
its light or dark theme, so a drawing in any other colour would break.

1. The root element declares `viewBox="0 0 <width> <height>"`, in whole numbers.
   The Pro 3 front is `viewBox="0 0 500 356"`.
2. Lines are `#000000`. The body fill is `#ffffff`. The only other paint is `none`.
   Paint may sit in a `fill` or `stroke` attribute or inside `style`.
   Other style properties such as `stroke-width` and `stroke-linejoin` are free.
3. Use only `svg`, `g`, `defs`, `path`, `circle`, `rect` and `ellipse`. No text,
   images, gradients, `class` attributes or `transform` attributes.
4. Leave out logos and trademarks. Draw a logo button as a plain shape.

The loader skips `title`, `metadata`, and every Inkscape or Sodipodi element, so a
file saved from Inkscape loads as is. An empty `defs` passes; anything inside one is
checked like the rest of the file.

A `transform` fails the load, because it would move a shape away from its hotspot.
Inkscape writes one in three cases. Set it up like this:

1. Keep Preferences > Behavior > Transforms > Store transformation on Optimized,
   the default. A moved `rect` or `circle` then gets new coordinates, not a
   transform.
2. Ungroup a group you moved. Ungrouping bakes the group's translate into its
   children.
3. Turn a rotated shape into a path (Path > Object to Path), then nudge it one step
   with an arrow key and back. The nudge bakes the rotation into the path data.

## 3. Mark each button's shape

Give each button's `path`, `circle` or `rect` the attribute
`data-button="<button id>"`, where the id is the button's `id` in
`description.json`:

```xml
<circle data-button="bottom face" cx="382" cy="146" r="15"
        fill="#ffffff" stroke="#000000" stroke-width="1.5"/>
```

That shape is the button's hotspot: the editor highlights it and tests clicks
against it. An `ellipse` cannot be a hotspot; draw it as a `path`.

The rules:

1. At most one hotspot per button per view. A button needs a hotspot in at least
   one view. A button on an edge may have one in each view: the Pro 3 has L4 and R4
   on both the front and the back, but L2 and R2 on the back only.
2. Every hotspot lies inside the `viewBox`.
3. A d-pad is one drawn cross with an invisible shape per direction over it. The
   invisible shapes carry `fill="none" stroke="none"`, so the centre of the cross
   belongs to no direction:

   ```xml
   <path d="M 105 76 L 131 76 L 131 101 L 156 101 L 156 127 L 131 127 L 131 152
            L 105 152 L 105 127 L 80 127 L 80 101 L 105 101 Z"
         fill="#ffffff" stroke="#000000" stroke-width="1.5"/>
   <rect data-button="d-pad up" x="105" y="76" width="26" height="25"
         fill="none" stroke="none"/>
   ```

4. Cut a shape that sits partly behind another to its visible outline. A bumper
   under the body, for example, ends where the body starts. Otherwise its highlight
   spills over the body.
5. On a back view, the controller's right side is on the left. The Pro 3 back has
   R4 at the left and L4 at the right.

## 4. List the views and embed the files

List each view in `description.json`, in display order:

```json
"views": [
  { "id": "front", "svg": "front.svg" },
  { "id": "back", "svg": "back.svg" }
]
```

Then embed each file in the model's `ControllerDescription::parse` call, in
`crates/controller-core/src/devices/<model>/mod.rs`:

```rust
ControllerDescription::parse(
    include_str!("../../../controllers/pro3/description.json"),
    &[
        ("front.svg", include_str!("../../../controllers/pro3/front.svg")),
        ("back.svg", include_str!("../../../controllers/pro3/back.svg")),
    ],
)
```

The file name in each pair matches the view's `svg` value.

## 5. Test the views

Run the crate's tests:

```sh
cargo test -p controller-core
```

A load error names the file and the fault, for example
`'front.svg': fill '#ff0000' is not a view colour`.

The loader cannot tell whether a hotspot covers the right part of the drawing.
Copy `pro3_hotspots_sit_on_their_buttons` from
`crates/controller-core/src/devices/pro3/mod.rs` and click real points: a button's
centre, the d-pad centre (no button), the body beside a bumper (no button), and a
button seen from behind. Pick the points from the coordinates in your SVG.
